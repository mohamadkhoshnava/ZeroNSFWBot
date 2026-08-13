"""FastAPI wrapper around the NSFW classifiers and OCR.

The bot is the only client. Two endpoints score images:

* ``/classify`` runs the fast model on everything.
* ``/verify`` runs the slower, more precise model on the small fraction the
  fast one flagged.

A third, ``/frames``, turns an animation or clip into a handful of stills the
other two can then score. It is separate rather than folded into ``/classify``
because the thresholds that decide what to do with those scores belong to the
bot, per group — the detector's job stops at "here is what this frame is".

Everything is best-effort per image: one corrupt avatar must never fail the
whole batch, because the bot would then have no signal at all and let a spammer
through.
"""

from __future__ import annotations

import base64
import binascii
import logging
from contextlib import asynccontextmanager

from fastapi import FastAPI, HTTPException
from fastapi.concurrency import run_in_threadpool

from . import config, frames, ocr
from .model import NsfwModel
from .schemas import (
    ClassifyRequest,
    ClassifyResponse,
    ClassifyResult,
    ExtractedFrame,
    FramesRequest,
    FramesResponse,
    FramesResult,
    HealthResponse,
    ModelInfo,
    ModelInfoResponse,
    OcrRequest,
    OcrResponse,
    OcrResult,
    VerifyResponse,
)

logging.basicConfig(
    level=config.LOG_LEVEL, format="%(asctime)s %(levelname)s %(name)s: %(message)s"
)
log = logging.getLogger("detector")

_fast: NsfwModel | None = None
_verifier: NsfwModel | None = None


@asynccontextmanager
async def lifespan(_app: FastAPI):
    global _fast, _verifier

    # Load eagerly so the container's healthcheck fails fast on a bad model
    # rather than surfacing as a timeout on the first real classification.
    _fast = NsfwModel(config.FAST_MODEL_DIR)

    if config.ENABLE_VERIFIER and (config.VERIFIER_MODEL_DIR / "model.onnx").exists():
        _verifier = NsfwModel(config.VERIFIER_MODEL_DIR)
    else:
        # Not fatal: the bot checks `available` and declines to act rather than
        # falling back to the fast model's opinion.
        log.warning("no verifier model loaded — /verify will report unavailable")

    log.info(
        "detector ready (verifier=%s, ocr=%s, video=%s)",
        _verifier is not None,
        ocr.available(),
        frames.video_available(),
    )
    yield
    _fast = _verifier = None


app = FastAPI(title="ZeroNSFW detector", version="2.0.0", lifespan=lifespan)


def _require_fast() -> NsfwModel:
    if _fast is None:  # pragma: no cover - lifespan guarantees this
        raise HTTPException(status_code=503, detail="model not loaded")
    return _fast


def _decode(data: str, limit: int | None = None) -> bytes:
    cap = config.MAX_IMAGE_BYTES if limit is None else limit
    raw = base64.b64decode(data, validate=True)
    if len(raw) > cap:
        raise ValueError(f"image exceeds {cap} bytes")
    return raw


def _score(model: NsfwModel, req: ClassifyRequest) -> list[ClassifyResult]:
    """Decode and score a batch, isolating per-image failures."""
    tensors = []
    pending: list[int] = []
    results: list[ClassifyResult | None] = [None] * len(req.images)

    for idx, item in enumerate(req.images):
        try:
            tensors.append(model.preprocess(_decode(item.data)))
            pending.append(idx)
        except (ValueError, binascii.Error) as exc:
            results[idx] = ClassifyResult(id=item.id, nsfw=0.0, sfw=0.0, error=str(exc))
        except Exception as exc:
            results[idx] = ClassifyResult(
                id=item.id, nsfw=0.0, sfw=0.0, error=f"decode failed: {exc}"
            )

    for idx, score in zip(pending, model.classify_batch(tensors)):
        results[idx] = ClassifyResult(
            id=req.images[idx].id,
            nsfw=round(score.nsfw, 6),
            sfw=round(1.0 - score.nsfw, 6),
            labels=score.labels,
        )

    return [r for r in results if r is not None]


def _info(model: NsfwModel) -> ModelInfo:
    return ModelInfo(
        model_id=model.meta.model_id,
        labels=model.meta.labels,
        nsfw_labels=model.meta.nsfw_labels,
        input_size=model.meta.input_size,
    )


@app.get("/health", response_model=HealthResponse)
async def health() -> HealthResponse:
    return HealthResponse(
        status="ok" if _fast is not None else "loading",
        model_loaded=_fast is not None,
        verifier_loaded=_verifier is not None,
        ocr_enabled=ocr.available(),
        video_enabled=frames.video_available(),
    )


@app.get("/model", response_model=ModelInfoResponse)
async def model_info() -> ModelInfoResponse:
    fast = _require_fast()
    return ModelInfoResponse(
        model_id=fast.meta.model_id,
        labels=fast.meta.labels,
        input_size=fast.meta.input_size,
        mean=fast.meta.mean,
        std=fast.meta.std,
        max_batch=config.MAX_BATCH,
        fast=_info(fast),
        verifier=_info(_verifier) if _verifier else None,
    )


@app.post("/classify", response_model=ClassifyResponse)
async def classify(req: ClassifyRequest) -> ClassifyResponse:
    model = _require_fast()

    if len(req.images) > config.MAX_BATCH:
        raise HTTPException(
            status_code=413,
            detail=f"batch of {len(req.images)} exceeds max {config.MAX_BATCH}",
        )
    if not req.images:
        return ClassifyResponse(results=[], model_id=model.meta.model_id)

    results = await run_in_threadpool(_score, model, req)
    return ClassifyResponse(results=results, model_id=model.meta.model_id)


@app.post("/verify", response_model=VerifyResponse)
async def verify(req: ClassifyRequest) -> VerifyResponse:
    """Second opinion from the heavier model.

    The fast model over-flags stylised art; this one has a `drawings` class and
    can say so. When no verifier is loaded the response is explicitly
    `available: false` — never a silent pass-through of the fast score, which
    would quietly reinstate the false positives this exists to prevent.
    """
    if _verifier is None:
        return VerifyResponse(results=[], model_id="", available=False)

    if len(req.images) > config.MAX_BATCH:
        raise HTTPException(status_code=413, detail="batch too large")
    if not req.images:
        return VerifyResponse(results=[], model_id=_verifier.meta.model_id)

    results = await run_in_threadpool(_score, _verifier, req)
    return VerifyResponse(results=results, model_id=_verifier.meta.model_id)


@app.post("/ocr", response_model=OcrResponse)
async def run_ocr(req: OcrRequest) -> OcrResponse:
    enabled = ocr.available()
    if not enabled or not req.images:
        return OcrResponse(
            results=[OcrResult(id=item.id, text="") for item in req.images],
            enabled=enabled,
        )

    if len(req.images) > config.MAX_BATCH:
        raise HTTPException(status_code=413, detail="batch too large")

    def work() -> list[OcrResult]:
        out: list[OcrResult] = []
        for item in req.images:
            try:
                out.append(
                    OcrResult(id=item.id, text=ocr.extract(_decode(item.data), req.langs))
                )
            except Exception as exc:
                out.append(OcrResult(id=item.id, text="", error=str(exc)))
        return out

    return OcrResponse(results=await run_in_threadpool(work), enabled=True)

"""FastAPI wrapper around the NSFW classifier and OCR.

The bot is the only client. Everything is best-effort per image: one corrupt
avatar must never fail the whole batch, because the bot would then have no
signal at all and let a spammer through.
"""

from __future__ import annotations

import base64
import binascii
import logging
from contextlib import asynccontextmanager

from fastapi import FastAPI, HTTPException
from fastapi.concurrency import run_in_threadpool

from . import config, ocr
from .model import NsfwModel
from .schemas import (
    ClassifyRequest,
    ClassifyResponse,
    ClassifyResult,
    HealthResponse,
    ModelInfoResponse,
    OcrRequest,
    OcrResponse,
    OcrResult,
)

logging.basicConfig(
    level=config.LOG_LEVEL, format="%(asctime)s %(levelname)s %(name)s: %(message)s"
)
log = logging.getLogger("detector")

_model: NsfwModel | None = None


@asynccontextmanager
async def lifespan(_app: FastAPI):
    global _model
    # Load eagerly so the container's healthcheck fails fast on a bad model
    # rather than surfacing as a timeout on the first real classification.
    _model = NsfwModel()
    log.info("detector ready (ocr=%s)", ocr.available())
    yield
    _model = None


app = FastAPI(title="ZeroNSFW detector", version="1.0.0", lifespan=lifespan)


def _require_model() -> NsfwModel:
    if _model is None:  # pragma: no cover - lifespan guarantees this
        raise HTTPException(status_code=503, detail="model not loaded")
    return _model


def _decode(data: str) -> bytes:
    raw = base64.b64decode(data, validate=True)
    if len(raw) > config.MAX_IMAGE_BYTES:
        raise ValueError(f"image exceeds {config.MAX_IMAGE_BYTES} bytes")
    return raw


@app.get("/health", response_model=HealthResponse)
async def health() -> HealthResponse:
    return HealthResponse(
        status="ok" if _model is not None else "loading",
        model_loaded=_model is not None,
        ocr_enabled=ocr.available(),
    )


@app.get("/model", response_model=ModelInfoResponse)
async def model_info() -> ModelInfoResponse:
    model = _require_model()
    return ModelInfoResponse(
        model_id=model.meta.model_id,
        labels=model.meta.labels,
        input_size=model.meta.input_size,
        mean=model.meta.mean,
        std=model.meta.std,
        max_batch=config.MAX_BATCH,
    )


@app.post("/classify", response_model=ClassifyResponse)
async def classify(req: ClassifyRequest) -> ClassifyResponse:
    model = _require_model()

    if len(req.images) > config.MAX_BATCH:
        raise HTTPException(
            status_code=413,
            detail=f"batch of {len(req.images)} exceeds max {config.MAX_BATCH}",
        )
    if not req.images:
        return ClassifyResponse(results=[], model_id=model.meta.model_id)

    def work() -> list[ClassifyResult]:
        # Decode failures are recorded per item and excluded from the batch so
        # the remaining images still get scored.
        tensors = []
        pending: list[int] = []
        results: list[ClassifyResult | None] = [None] * len(req.images)

        for idx, item in enumerate(req.images):
            try:
                tensors.append(model.preprocess(_decode(item.data)))
                pending.append(idx)
            except (ValueError, binascii.Error) as exc:
                results[idx] = ClassifyResult(
                    id=item.id, nsfw=0.0, sfw=0.0, error=str(exc)
                )
            except Exception as exc:
                results[idx] = ClassifyResult(
                    id=item.id, nsfw=0.0, sfw=0.0, error=f"decode failed: {exc}"
                )

        for idx, (nsfw, sfw) in zip(pending, model.classify_batch(tensors)):
            results[idx] = ClassifyResult(
                id=req.images[idx].id, nsfw=round(nsfw, 6), sfw=round(sfw, 6)
            )

        return [r for r in results if r is not None]

    results = await run_in_threadpool(work)
    return ClassifyResponse(results=results, model_id=model.meta.model_id)


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

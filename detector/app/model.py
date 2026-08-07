"""ONNX inference for the NSFW classifier.

Loaded once at startup. `classify_batch` is CPU-bound and synchronous; callers
must dispatch it to a worker thread so the event loop stays responsive.
"""

from __future__ import annotations

import io
import json
import logging
from dataclasses import dataclass

import numpy as np
import onnxruntime as ort
from PIL import Image, ImageFile

from . import config

log = logging.getLogger(__name__)

# Telegram occasionally serves slightly truncated JPEGs; decoding what we got
# beats dropping the check entirely.
ImageFile.LOAD_TRUNCATED_IMAGES = True
Image.MAX_IMAGE_PIXELS = config.MAX_IMAGE_PIXELS

_RESAMPLE = {
    "bicubic": Image.Resampling.BICUBIC,
    "bilinear": Image.Resampling.BILINEAR,
    "nearest": Image.Resampling.NEAREST,
    "lanczos": Image.Resampling.LANCZOS,
}


@dataclass(frozen=True)
class Metadata:
    model_id: str
    labels: list[str]
    input_size: list[int]
    mean: list[float]
    std: list[float]
    interpolation: str


class NsfwModel:
    def __init__(self) -> None:
        self.meta = self._load_metadata()
        self.session = self._load_session()

        _, self.height, self.width = self.meta.input_size
        self._mean = np.array(self.meta.mean, dtype=np.float32).reshape(3, 1, 1)
        self._std = np.array(self.meta.std, dtype=np.float32).reshape(3, 1, 1)
        self._resample = _RESAMPLE.get(
            self.meta.interpolation, Image.Resampling.BICUBIC
        )
        self._input_name = self.session.get_inputs()[0].name

        # Index of the NSFW class, matched case-insensitively so a differently
        # cased label in the model config does not silently invert the score.
        self._nsfw_idx = next(
            (i for i, name in enumerate(self.meta.labels) if "nsfw" in name.lower()),
            0,
        )
        self._sfw_idx = 1 - self._nsfw_idx if len(self.meta.labels) == 2 else None

        log.info(
            "loaded %s (labels=%s, nsfw_index=%d, input=%dx%d)",
            self.meta.model_id,
            self.meta.labels,
            self._nsfw_idx,
            self.height,
            self.width,
        )

    @staticmethod
    def _load_metadata() -> Metadata:
        raw = json.loads(config.METADATA_PATH.read_text())
        return Metadata(
            model_id=raw["model_id"],
            labels=raw["labels"],
            input_size=raw["input_size"],
            mean=raw["mean"],
            std=raw["std"],
            interpolation=raw.get("interpolation", "bicubic"),
        )

    @staticmethod
    def _load_session() -> ort.InferenceSession:
        opts = ort.SessionOptions()
        opts.intra_op_num_threads = config.THREADS
        opts.inter_op_num_threads = 1
        opts.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL
        return ort.InferenceSession(
            str(config.MODEL_PATH), sess_options=opts, providers=["CPUExecutionProvider"]
        )

    def preprocess(self, raw: bytes) -> np.ndarray:
        """Decode and normalise one image into a CHW float32 tensor."""
        with Image.open(io.BytesIO(raw)) as img:
            # `convert` also flattens alpha, which would otherwise leave a
            # 4-channel array the model cannot consume.
            img = img.convert("RGB").resize(
                (self.width, self.height), self._resample
            )
            arr = np.asarray(img, dtype=np.float32) / 255.0

        arr = np.transpose(arr, (2, 0, 1))
        return (arr - self._mean) / self._std

    def classify_batch(self, tensors: list[np.ndarray]) -> list[tuple[float, float]]:
        """Run one inference over a pre-processed batch.

        Returns (nsfw, sfw) probabilities per input, in the same order.
        """
        if not tensors:
            return []

        batch = np.stack(tensors).astype(np.float32)
        logits = self.session.run(None, {self._input_name: batch})[0]
        probs = _softmax(logits)

        out: list[tuple[float, float]] = []
        for row in probs:
            nsfw = float(row[self._nsfw_idx])
            sfw = (
                float(row[self._sfw_idx])
                if self._sfw_idx is not None
                else float(1.0 - nsfw)
            )
            out.append((nsfw, sfw))
        return out


def _softmax(logits: np.ndarray) -> np.ndarray:
    shifted = logits - np.max(logits, axis=-1, keepdims=True)
    exp = np.exp(shifted)
    return exp / np.sum(exp, axis=-1, keepdims=True)

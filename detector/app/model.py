"""ONNX inference for the NSFW classifiers.

Two models are loaded: a fast screening model and a slower, more precise
verifier. Both use this class — they differ only in weights, input size and
label taxonomy, all of which come from the exported ``metadata.json``.

`classify_batch` is CPU-bound and synchronous; callers must dispatch it to a
worker thread so the event loop stays responsive.
"""

from __future__ import annotations

import io
import json
import logging
from dataclasses import dataclass
from pathlib import Path

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
    nsfw_labels: list[str]
    input_size: list[int]
    mean: list[float]
    std: list[float]
    interpolation: str


@dataclass(frozen=True)
class Score:
    """One image's result: the NSFW total plus the per-label breakdown.

    The breakdown is what makes a multi-class verifier worth its cost — it can
    say "this is a drawing, not pornography", and an operator can see why.
    """

    nsfw: float
    labels: dict[str, float]


class NsfwModel:
    def __init__(self, model_dir: Path) -> None:
        self.dir = model_dir
        self.meta = self._load_metadata(model_dir / "metadata.json")
        self.session = self._load_session(model_dir / "model.onnx")

        _, self.height, self.width = self.meta.input_size
        self._mean = np.array(self.meta.mean, dtype=np.float32).reshape(3, 1, 1)
        self._std = np.array(self.meta.std, dtype=np.float32).reshape(3, 1, 1)
        self._resample = _RESAMPLE.get(
            self.meta.interpolation, Image.Resampling.BICUBIC
        )
        self._input_name = self.session.get_inputs()[0].name

        # Indices whose probabilities sum to the NSFW score. Matched by name so
        # a reordered or renamed class cannot silently invert the verdict.
        lowered = [label.lower() for label in self.meta.labels]
        self._nsfw_idx = [
            lowered.index(label.lower())
            for label in self.meta.nsfw_labels
            if label.lower() in lowered
        ]
        if not self._nsfw_idx:
            raise ValueError(
                f"{model_dir}: none of {self.meta.nsfw_labels} are in "
                f"{self.meta.labels}; every image would score 0"
            )

        log.info(
            "loaded %s from %s (labels=%s, nsfw=%s, input=%dx%d)",
            self.meta.model_id,
            model_dir,
            self.meta.labels,
            self.meta.nsfw_labels,
            self.height,
            self.width,
        )

    @staticmethod
    def _load_metadata(path: Path) -> Metadata:
        raw = json.loads(path.read_text())
        labels = raw["labels"]
        return Metadata(
            model_id=raw["model_id"],
            labels=labels,
            # Older exports predate the field; treat everything non-safe as NSFW.
            nsfw_labels=raw.get(
                "nsfw_labels",
                [l for l in labels if l.lower() not in {"sfw", "normal", "neutral"}],
            ),
            input_size=raw["input_size"],
            mean=raw["mean"],
            std=raw["std"],
            interpolation=raw.get("interpolation", "bicubic"),
        )

    @staticmethod
    def _load_session(path: Path) -> ort.InferenceSession:
        opts = ort.SessionOptions()
        opts.intra_op_num_threads = config.THREADS
        opts.inter_op_num_threads = 1
        opts.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL
        return ort.InferenceSession(
            str(path), sess_options=opts, providers=["CPUExecutionProvider"]
        )

    def preprocess(self, raw: bytes) -> np.ndarray:
        """Decode and normalise one image into a CHW float32 tensor."""
        with Image.open(io.BytesIO(raw)) as img:
            # `convert` also flattens alpha, which would otherwise leave a
            # 4-channel array the model cannot consume.
            img = img.convert("RGB").resize((self.width, self.height), self._resample)
            arr = np.asarray(img, dtype=np.float32) / 255.0

        arr = np.transpose(arr, (2, 0, 1))
        return (arr - self._mean) / self._std

    def classify_batch(self, tensors: list[np.ndarray]) -> list[Score]:
        """Run one inference over a pre-processed batch, in input order."""
        if not tensors:
            return []

        batch = np.stack(tensors).astype(np.float32)
        logits = self.session.run(None, {self._input_name: batch})[0]
        probs = _softmax(logits)

        out: list[Score] = []
        for row in probs:
            out.append(
                Score(
                    nsfw=float(sum(row[i] for i in self._nsfw_idx)),
                    labels={
                        label: round(float(row[i]), 6)
                        for i, label in enumerate(self.meta.labels)
                    },
                )
            )
        return out


def _softmax(logits: np.ndarray) -> np.ndarray:
    shifted = logits - np.max(logits, axis=-1, keepdims=True)
    exp = np.exp(shifted)
    return exp / np.sum(exp, axis=-1, keepdims=True)

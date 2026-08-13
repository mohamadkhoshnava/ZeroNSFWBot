"""Request/response models shared with the Rust bot.

Any change here must be mirrored in `bot/src/detector/mod.rs`.
"""

from __future__ import annotations

from pydantic import BaseModel, Field


class ImageItem(BaseModel):
    # Caller-chosen correlation id. The bot passes Telegram's `file_unique_id`
    # so results can be cached without re-hashing the bytes.
    id: str
    # Base64-encoded image bytes (no data: prefix).
    data: str


class ClassifyRequest(BaseModel):
    images: list[ImageItem] = Field(default_factory=list)


class ClassifyResult(BaseModel):
    id: str
    # Probability in [0, 1] that the image is NSFW. For a multi-class model this
    # is the sum of the labels the export marked as explicit.
    nsfw: float
    sfw: float
    # Per-label probabilities. Empty for a binary model, and the whole point of
    # the verifier for a multi-class one: it shows *why*, e.g. drawings=0.91.
    labels: dict[str, float] = Field(default_factory=dict)
    # Present instead of scores when this one image failed to decode; the rest
    # of the batch still succeeds.
    error: str | None = None


class ClassifyResponse(BaseModel):
    results: list[ClassifyResult]
    model_id: str


class VerifyResponse(ClassifyResponse):
    # False when the image was built without a verifier. The bot must not treat
    # an unverified image as confirmed.
    available: bool = True


class OcrRequest(BaseModel):
    images: list[ImageItem] = Field(default_factory=list)
    langs: str | None = None


class OcrResult(BaseModel):
    id: str
    text: str
    error: str | None = None


class OcrResponse(BaseModel):
    results: list[OcrResult]
    enabled: bool


class HealthResponse(BaseModel):
    status: str
    model_loaded: bool
    verifier_loaded: bool
    ocr_enabled: bool


class ModelInfo(BaseModel):
    model_id: str
    labels: list[str]
    nsfw_labels: list[str]
    input_size: list[int]


class ModelInfoResponse(BaseModel):
    # Kept at the top level for backwards compatibility with the eval script.
    model_id: str
    labels: list[str]
    input_size: list[int]
    mean: list[float]
    std: list[float]
    max_batch: int
    fast: ModelInfo
    verifier: ModelInfo | None = None

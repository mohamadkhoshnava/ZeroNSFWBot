"""Request/response models shared with the Rust bot.

Any change here must be mirrored in `bot/src/detector/types.rs`.
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
    # Probability in [0, 1] that the image is NSFW.
    nsfw: float
    sfw: float
    # Present instead of scores when this one image failed to decode; the rest
    # of the batch still succeeds.
    error: str | None = None


class ClassifyResponse(BaseModel):
    results: list[ClassifyResult]
    model_id: str


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
    ocr_enabled: bool


class ModelInfoResponse(BaseModel):
    model_id: str
    labels: list[str]
    input_size: list[int]
    mean: list[float]
    std: list[float]
    max_batch: int

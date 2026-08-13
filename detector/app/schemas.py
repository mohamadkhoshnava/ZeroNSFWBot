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


class FramesRequest(BaseModel):
    # Whole media files (GIF, MP4, WebM, WebP, or an ordinary still), base64
    # encoded. Same shape as ClassifyRequest so the bot reuses one item type.
    images: list[ImageItem] = Field(default_factory=list)
    # How many stills to spread over each clip. Clamped to DETECTOR_MAX_FRAMES.
    max_frames: int = 5


class ExtractedFrame(BaseModel):
    # "<item id>#<n>", so the caller can feed these straight to /classify and
    # still trace a score back to the clip it came from.
    id: str
    # Position in the source, 0-based.
    index: int
    # Base64-encoded JPEG.
    data: str


class FramesResult(BaseModel):
    id: str
    frames: list[ExtractedFrame] = Field(default_factory=list)
    # Frames in the source, 0 when the decoder could not say.
    total: int = 0
    # Which decoder ran: "still", "pillow" or "ffmpeg".
    decoder: str = ""
    # Set instead of frames when this one file failed; the batch survives.
    error: str | None = None


class FramesResponse(BaseModel):
    results: list[FramesResult]
    # False when this build cannot decode video containers at all, so the
    # caller can fall back to Telegram's thumbnail instead of assuming the
    # clip was checked.
    video_enabled: bool = True


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
    # Whether ffmpeg is present, i.e. whether /frames can sample real video.
    video_enabled: bool = False


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

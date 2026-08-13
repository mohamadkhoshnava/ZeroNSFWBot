"""Runtime configuration, all sourced from the environment."""

from __future__ import annotations

import os
from pathlib import Path


def _bool(name: str, default: bool) -> bool:
    raw = os.environ.get(name)
    if raw is None:
        return default
    return raw.strip().lower() in {"1", "true", "yes", "on"}


def _int(name: str, default: int) -> int:
    try:
        return int(os.environ[name])
    except (KeyError, ValueError):
        return default


MODEL_DIR = Path(os.environ.get("MODEL_DIR", "/models"))
# The fast screening model, run on every scanned image.
FAST_MODEL_DIR = MODEL_DIR / "fast"
# The slower, more precise model, run only on what the fast one flags.
VERIFIER_MODEL_DIR = MODEL_DIR / "verifier"

# The verifier can be built out of the image entirely; when it is missing,
# /verify says so rather than pretending to have checked.
ENABLE_VERIFIER = _bool("ENABLE_VERIFIER", True)

THREADS = _int("DETECTOR_THREADS", 2)

ENABLE_OCR = _bool("ENABLE_OCR", True)
OCR_LANGS = os.environ.get("OCR_LANGS", "eng+fas+ara+rus")

# Frame sampling for animations and clips. A GIF is only NSFW for part of its
# run often enough that scoring the first frame — or the poster thumbnail
# Telegram hands out — misses it entirely, so /frames spreads its samples over
# the whole clip. Multi-frame stills (GIF, animated WebP, APNG) go through
# Pillow; real video containers need ffmpeg, which the image installs unless
# ENABLE_VIDEO was false at build time.
ENABLE_VIDEO = _bool("ENABLE_VIDEO", True)
MAX_FRAMES = _int("DETECTOR_MAX_FRAMES", 12)
# Frames are handed straight to a 384px model, so anything larger is wasted
# bytes on the wire and wasted pixels in the resize.
FRAME_MAX_SIDE = _int("DETECTOR_FRAME_MAX_SIDE", 512)
FFMPEG_TIMEOUT = _int("DETECTOR_FFMPEG_TIMEOUT_SECS", 20)

# Guardrails so a hostile or malformed request cannot exhaust memory.
MAX_BATCH = _int("DETECTOR_MAX_BATCH", 16)
MAX_IMAGE_BYTES = _int("DETECTOR_MAX_IMAGE_BYTES", 12 * 1024 * 1024)
# Clips are whole files rather than single stills, so they get their own,
# larger cap — 20 MB is the most Telegram's getFile will ever serve anyway.
MAX_MEDIA_BYTES = _int("DETECTOR_MAX_MEDIA_BYTES", 20 * 1024 * 1024)
# Pillow refuses to decode images with more pixels than this (decompression
# bombs). 80MP is far above anything Telegram will ever hand us.
MAX_IMAGE_PIXELS = _int("DETECTOR_MAX_IMAGE_PIXELS", 80_000_000)

LOG_LEVEL = os.environ.get("LOG_LEVEL", "info").upper()

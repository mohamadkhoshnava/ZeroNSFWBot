"""Optional OCR of profile pictures.

Spammers routinely burn their @username or a t.me link into the avatar itself,
where no text-field filter can see it. Reading that text turns the avatar into
another contact-info signal for the `profile_ocr` filter.
"""

from __future__ import annotations

import io
import logging

from PIL import Image, ImageOps

from . import config

log = logging.getLogger(__name__)

try:  # pytesseract is installed even when OCR is off; the binary may not be.
    import pytesseract

    _IMPORT_OK = True
except ImportError:  # pragma: no cover - only hit in stripped builds
    pytesseract = None  # type: ignore[assignment]
    _IMPORT_OK = False


def available() -> bool:
    if not (config.ENABLE_OCR and _IMPORT_OK):
        return False
    try:
        pytesseract.get_tesseract_version()
        return True
    except Exception as exc:  # pragma: no cover - environment dependent
        log.warning("OCR requested but tesseract is unusable: %s", exc)
        return False


def extract(raw: bytes, langs: str | None = None) -> str:
    """Best-effort text extraction. Returns "" rather than raising."""
    if not available():
        return ""

    try:
        with Image.open(io.BytesIO(raw)) as img:
            prepared = _prepare(img)
        text = pytesseract.image_to_string(
            prepared, lang=langs or config.OCR_LANGS
        )
    except Exception as exc:
        log.debug("OCR failed: %s", exc)
        return ""

    return " ".join(text.split())


def _prepare(img: Image.Image) -> Image.Image:
    """Grayscale, autocontrast and upscale small avatars.

    Telegram avatars are commonly 160px, well below the ~300 DPI tesseract
    wants; upscaling recovers a large share of otherwise-missed overlays.
    """
    gray = ImageOps.autocontrast(img.convert("L"))
    if max(gray.size) < 640:
        scale = 640 / max(gray.size)
        gray = gray.resize(
            (int(gray.width * scale), int(gray.height * scale)),
            Image.Resampling.LANCZOS,
        )
    return gray

"""Sampling still frames out of animations and clips.

A GIF that is explicit for half a second and innocuous either side of it is the
whole reason this exists: the bot used to score Telegram's poster thumbnail,
which is one arbitrary frame and very often the wrong one. Here a clip is
reduced to a handful of stills spread across its full length, and the caller
scores every one of them and keeps the worst.

Two decoders, picked by sniffing the container:

* Pillow for multi-frame stills — GIF, animated WebP, APNG. No subprocess, and
  it is already a dependency.
* ffmpeg for real video — MP4 (which is what Telegram converts uploaded GIFs
  into), WebM video stickers, Matroska. Optional: when the binary is absent the
  caller is told so rather than silently receiving one frame.

Everything is best-effort and bounded: a hostile file gets a wall-clock limit,
a pixel limit and a frame limit, and any failure returns an error for that one
item instead of raising.
"""

from __future__ import annotations

import io
import logging
import shutil
import subprocess
import tempfile
from dataclasses import dataclass
from pathlib import Path

from PIL import Image, ImageFile, ImageSequence

from . import config

log = logging.getLogger(__name__)

ImageFile.LOAD_TRUNCATED_IMAGES = True
Image.MAX_IMAGE_PIXELS = config.MAX_IMAGE_PIXELS


@dataclass(frozen=True)
class Frame:
    """One sampled still, JPEG-encoded."""

    # Position in the source, 0-based. Reported back so a detection can say
    # *which* part of the clip was explicit.
    index: int
    data: bytes


@dataclass(frozen=True)
class Sampled:
    frames: list[Frame]
    # Frames in the source, as far as the decoder could tell. 0 when unknown
    # (a stream without a frame count), 1 for an ordinary still image.
    total: int
    # Which decoder produced this: "still", "pillow" or "ffmpeg".
    decoder: str


def video_available() -> bool:
    """Whether real video containers can be decoded at all."""
    return config.ENABLE_VIDEO and _ffmpeg() is not None and _ffprobe() is not None


def sample(raw: bytes, max_frames: int) -> Sampled:
    """Reduce one media file to at most `max_frames` stills.

    Raises `ValueError` for input this build cannot decode — a video when
    ffmpeg is missing, or bytes that are not media at all.
    """
    limit = max(1, min(max_frames, config.MAX_FRAMES))
    kind = sniff(raw)

    if kind == "video":
        if not video_available():
            raise ValueError("video decoding is unavailable in this build")
        return _from_video(raw, limit)

    # Animated stills and ordinary images share a decoder; `n_frames` is what
    # tells them apart, so there is no need to trust the sniffed type here.
    return _from_pillow(raw, limit)


def sniff(raw: bytes) -> str:
    """Container family: "video" for anything needing ffmpeg, else "image"."""
    head = raw[:16]

    # ISO base media (MP4, M4V, MOV): "....ftyp" at offset 4.
    if len(head) >= 12 and head[4:8] == b"ftyp":
        return "video"
    # Matroska / WebM.
    if head.startswith(b"\x1a\x45\xdf\xa3"):
        return "video"
    return "image"


# ------------------------------------------------------------------ pillow --


def _from_pillow(raw: bytes, limit: int) -> Sampled:
    frames: list[Frame] = []

    with Image.open(io.BytesIO(raw)) as img:
        total = int(getattr(img, "n_frames", 1) or 1)

        for index in _spread(total, limit):
            try:
                img.seek(index)
            except EOFError:  # pragma: no cover - lying n_frames
                break
            # `copy` detaches from the sequence cursor, so encoding cannot be
            # disturbed by the next seek.
            frames.append(Frame(index=index, data=_encode(img.convert("RGB").copy())))

    if not frames:
        raise ValueError("no decodable frames")

    return Sampled(frames=frames, total=total, decoder="pillow" if total > 1 else "still")


# ------------------------------------------------------------------ ffmpeg --


def _from_video(raw: bytes, limit: int) -> Sampled:
    if len(raw) > config.MAX_MEDIA_BYTES:
        raise ValueError(f"clip exceeds {config.MAX_MEDIA_BYTES} bytes")

    with tempfile.TemporaryDirectory(prefix="frames-") as tmp:
        work = Path(tmp)
        source = work / "clip"
        source.write_bytes(raw)

        duration = _duration(source)

        # An fps filter below 1 emits the first frame of each interval, which
        # spreads `limit` samples over the whole clip in a single decode pass.
        # Without a duration to divide by, fall back to one frame per second
        # and simply take the first `limit` of them.
        fps = limit / duration if duration and duration > 0 else 1.0
        fps = min(max(fps, 0.01), 30.0)

        pattern = work / "f%03d.jpg"
        # The quotes around min(...) are ffmpeg's own filtergraph quoting: they
        # stop the comma being read as a filter separator. Nothing goes near a
        # shell — subprocess is given an argument list.
        scale = f"scale='min({config.FRAME_MAX_SIDE},iw)':-2"
        command = [
            _ffmpeg(),
            "-nostdin",
            "-v", "error",
            "-i", str(source),
            "-map", "0:v:0",
            "-an", "-sn", "-dn",
            "-vf", f"fps={fps:.6f},{scale}",
            "-frames:v", str(limit),
            "-f", "image2",
            str(pattern),
        ]

        try:
            subprocess.run(
                command,
                check=True,
                capture_output=True,
                timeout=config.FFMPEG_TIMEOUT,
            )
        except subprocess.TimeoutExpired as exc:
            raise ValueError(f"decoding timed out after {config.FFMPEG_TIMEOUT}s") from exc
        except subprocess.CalledProcessError as exc:
            detail = exc.stderr.decode("utf-8", "replace").strip()[:200]
            raise ValueError(f"ffmpeg failed: {detail or exc.returncode}") from exc

        files = sorted(work.glob("f*.jpg"))
        if not files:
            raise ValueError("ffmpeg produced no frames")

        # The index is the sample number, not the source frame number: with a
        # sub-1 fps filter the two are unrelated, and "frame 3 of 5 sampled" is
        # what a moderator can actually act on.
        frames = [Frame(index=i, data=path.read_bytes()) for i, path in enumerate(files)]

    return Sampled(frames=frames, total=len(frames), decoder="ffmpeg")


def _duration(path: Path) -> float | None:
    command = [
        _ffprobe(),
        "-v", "error",
        "-show_entries", "format=duration",
        "-of", "default=noprint_wrappers=1:nokey=1",
        str(path),
    ]
    try:
        out = subprocess.run(
            command, check=True, capture_output=True, timeout=config.FFMPEG_TIMEOUT
        )
        return float(out.stdout.decode().strip())
    except (subprocess.SubprocessError, ValueError) as exc:
        log.debug("could not probe duration: %s", exc)
        return None


def _ffmpeg() -> str | None:
    return shutil.which("ffmpeg")


def _ffprobe() -> str | None:
    return shutil.which("ffprobe")


# ------------------------------------------------------------------ shared --


def _spread(total: int, limit: int) -> list[int]:
    """`limit` indices spread evenly over `0..total`, endpoints included.

    Endpoints matter: spam GIFs routinely open on an innocuous frame, and a
    sampler that skipped the tail would miss the ones that end on one.
    """
    if total <= 1 or limit <= 1:
        return [0]
    if total <= limit:
        return list(range(total))

    step = (total - 1) / (limit - 1)
    return sorted({round(i * step) for i in range(limit)})


def _encode(img: Image.Image) -> bytes:
    if max(img.size) > config.FRAME_MAX_SIDE:
        scale = config.FRAME_MAX_SIDE / max(img.size)
        img = img.resize(
            (max(1, int(img.width * scale)), max(1, int(img.height * scale))),
            Image.Resampling.BILINEAR,
        )

    buffer = io.BytesIO()
    img.save(buffer, format="JPEG", quality=88)
    return buffer.getvalue()

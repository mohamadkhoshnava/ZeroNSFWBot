#!/usr/bin/env python3
"""Generate the committed safe-for-work sample images.

These exist so `scripts/eval_images.sh` and the detector tests have something to
run against out of the box, and so a fresh clone can verify the model is wired
up before anyone hunts for real test material.

They are synthetic on purpose: shapes, gradients and text, all obviously SFW.
Anything resembling real NSFW test data belongs in `test_images/nsfw/`, which is
gitignored — see the README there.
"""

from __future__ import annotations

import math
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

OUT_DIR = Path(__file__).parent / "test_images" / "sfw"
SIZE = (512, 512)


def gradient(name: str, start: tuple[int, int, int], end: tuple[int, int, int]) -> None:
    img = Image.new("RGB", SIZE)
    draw = ImageDraw.Draw(img)
    for y in range(SIZE[1]):
        t = y / (SIZE[1] - 1)
        draw.line(
            [(0, y), (SIZE[0], y)],
            fill=tuple(round(a + (b - a) * t) for a, b in zip(start, end)),
        )
    img.save(OUT_DIR / name)


def shapes(name: str) -> None:
    img = Image.new("RGB", SIZE, (245, 245, 240))
    draw = ImageDraw.Draw(img)
    draw.ellipse([60, 60, 240, 240], fill=(70, 130, 180))
    draw.rectangle([280, 90, 450, 260], fill=(210, 105, 30))
    draw.polygon([(256, 300), (150, 460), (362, 460)], fill=(60, 160, 90))
    img.save(OUT_DIR / name)


def checkerboard(name: str, cell: int = 64) -> None:
    img = Image.new("RGB", SIZE, (255, 255, 255))
    draw = ImageDraw.Draw(img)
    for row in range(SIZE[1] // cell):
        for col in range(SIZE[0] // cell):
            if (row + col) % 2 == 0:
                draw.rectangle(
                    [col * cell, row * cell, (col + 1) * cell, (row + 1) * cell],
                    fill=(40, 40, 40),
                )
    img.save(OUT_DIR / name)


def load_font(size: int) -> ImageFont.ImageFont:
    """A font tesseract can actually read.

    Upscaling PIL's tiny bitmap default produces glyphs OCR cannot resolve, so
    prefer a real TrueType face and fall back to the scalable default that
    Pillow ≥10.1 provides.
    """
    for path in (
        "/System/Library/Fonts/Supplemental/Arial.ttf",
        "/System/Library/Fonts/Helvetica.ttc",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    ):
        try:
            return ImageFont.truetype(path, size)
        except OSError:
            continue

    try:
        return ImageFont.load_default(size=size)
    except TypeError:  # Pillow < 10.1
        return ImageFont.load_default()


def text_overlay(name: str, caption: str) -> None:
    """A plain image with text, to sanity-check the OCR endpoint.

    `caption` deliberately looks like the contact info spammers burn into their
    avatars, so `/ocr` and the `profile_ocr` filter can be exercised end to end
    without any NSFW material at all.
    """
    img = Image.new("RGB", SIZE, (245, 245, 245))
    draw = ImageDraw.Draw(img)

    # A faint background, so the probe is not a pure-white page — but far
    # lighter than the text, because OCR needs the contrast.
    for i in range(0, SIZE[0], 48):
        shade = 225 + int(12 * math.sin(i / 60))
        draw.line([(i, 0), (i, SIZE[1])], fill=(shade, shade, shade))

    font = load_font(56)
    box = draw.textbbox((0, 0), caption, font=font)
    draw.text(
        ((SIZE[0] - (box[2] - box[0])) / 2, (SIZE[1] - (box[3] - box[1])) / 2),
        caption,
        fill=(15, 15, 15),
        font=font,
    )

    img.save(OUT_DIR / name)


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)

    gradient("gradient_blue.png", (20, 40, 90), (180, 210, 250))
    gradient("gradient_warm.png", (120, 30, 20), (250, 220, 150))
    shapes("shapes.png")
    checkerboard("checkerboard.png")
    text_overlay("ocr_probe.png", "join t.me/example")

    written = sorted(p.name for p in OUT_DIR.glob("*.png"))
    print(f"wrote {len(written)} sample(s) to {OUT_DIR}:")
    for name in written:
        print(f"  • {name}")


if __name__ == "__main__":
    main()

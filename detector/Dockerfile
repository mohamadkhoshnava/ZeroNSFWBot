# ---------------------------------------------------------------------------
# Stage 1 — export the model to ONNX.
# torch + timm are ~1 GB; none of it reaches the final image.
# ---------------------------------------------------------------------------
FROM python:3.14-slim AS exporter

# The fast screening model, run on every image the bot scans.
ARG MODEL_ID=Marqo/nsfw-image-detection-384
# The verifier, run only on what the fast model flags. Its value is the
# taxonomy, not just the extra parameters: `drawings` and `sexy` are separate
# classes from `hentai` and `porn`, so an ordinary anime avatar stops being a
# ban. Set to an empty string to build without it.
ARG VERIFIER_MODEL_ID=giacomoarienti/nsfw-classifier
# Generous retries: torch is a large download and the models come from the HF
# hub, so a flaky link should slow the build down, not break it.
# HF_HUB_DISABLE_XET: the hub's default xet/CAS transfer path fails hard on an
# unreliable link — a 350 MB weight download dies mid-stream with a CAS client
# error rather than retrying. The classic HTTP path resumes properly.
ENV PIP_NO_CACHE_DIR=1 PIP_RETRIES=15 PIP_TIMEOUT=180 \
    HF_HUB_DISABLE_TELEMETRY=1 \
    HF_HUB_DISABLE_XET=1 \
    HF_HUB_ENABLE_HF_TRANSFER=0

WORKDIR /build
COPY requirements-build.txt .
RUN pip install --no-cache-dir -r requirements-build.txt

COPY scripts/export_onnx.py scripts/
RUN python scripts/export_onnx.py --model-id "${MODEL_ID}" --out-dir /models/fast
RUN if [ -n "${VERIFIER_MODEL_ID}" ]; then \
      python scripts/export_onnx.py \
        --model-id "${VERIFIER_MODEL_ID}" --out-dir /models/verifier; \
    else \
      echo "no verifier requested; /verify will report unavailable"; \
    fi

# ---------------------------------------------------------------------------
# Stage 2 — runtime. onnxruntime only.
# ---------------------------------------------------------------------------
FROM python:3.14-slim AS runtime

ARG ENABLE_OCR=true
# ffmpeg, for sampling frames out of GIFs-as-MP4 and video stickers. Telegram
# re-encodes every uploaded GIF to MP4, so without it /frames can only handle
# true GIF/WebP/APNG bytes and the bot falls back to the poster thumbnail —
# which is one arbitrary frame of the clip. Costs ~150 MB; set false to skip.
ARG ENABLE_VIDEO=true
ENV PIP_NO_CACHE_DIR=1 \
    PIP_RETRIES=10 \
    PIP_TIMEOUT=60 \
    PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1 \
    MODEL_DIR=/models

# tesseract + the language packs the bot's four locales need, and ffmpeg. Both
# are optional and skipped entirely when their flag is false, which is what
# keeps a stripped build small.
#
# The retry loop is not paranoia: this pulls tens of MB, and over a VPN or a
# throttled link apt reliably gets a truncated response part-way through.
# Without it a transient drop fails the whole build.
RUN set -eu; \
    packages=""; \
    if [ "${ENABLE_OCR}" = "true" ]; then \
      packages="tesseract-ocr tesseract-ocr-eng tesseract-ocr-fas tesseract-ocr-ara tesseract-ocr-rus"; \
    fi; \
    if [ "${ENABLE_VIDEO}" = "true" ]; then \
      packages="${packages} ffmpeg"; \
    fi; \
    if [ -n "${packages}" ]; then \
      printf 'Acquire::Retries "10";\nAcquire::http::Timeout "60";\nAcquire::https::Timeout "60";\n' \
        > /etc/apt/apt.conf.d/99-retries; \
      for attempt in 1 2 3 4 5; do \
        apt-get update -qq \
        && apt-get install -y --no-install-recommends --fix-missing ${packages} \
        && break; \
        echo "apt attempt ${attempt} failed, retrying"; sleep 10; \
      done; \
      for pkg in ${packages}; do dpkg -s "${pkg}" > /dev/null; done; \
      rm -rf /var/lib/apt/lists/*; \
    fi

WORKDIR /app
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY --from=exporter /models /models
COPY app ./app

# Run unprivileged: this service parses untrusted images from strangers.
RUN useradd --create-home --uid 10001 detector && chown -R detector /models /app
USER detector

EXPOSE 8000
CMD ["uvicorn", "app.main:app", "--host", "0.0.0.0", "--port", "8000", "--workers", "1"]

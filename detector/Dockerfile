# syntax=docker/dockerfile:1

# ---------------------------------------------------------------------------
# Stage 1 — export the model to ONNX.
# torch + timm are ~1 GB; none of it reaches the final image.
# ---------------------------------------------------------------------------
FROM python:3.12-slim AS exporter

ARG MODEL_ID=Marqo/nsfw-image-detection-384
# Generous retries: torch is a large download and the model comes from the HF
# hub, so a flaky link should slow the build down, not break it.
ENV PIP_NO_CACHE_DIR=1 PIP_RETRIES=10 PIP_TIMEOUT=60 HF_HUB_DISABLE_TELEMETRY=1

WORKDIR /build
COPY requirements-build.txt .
RUN pip install --no-cache-dir -r requirements-build.txt

COPY scripts/export_onnx.py scripts/
RUN python scripts/export_onnx.py --model-id "${MODEL_ID}" --out-dir /models

# ---------------------------------------------------------------------------
# Stage 2 — runtime. onnxruntime only.
# ---------------------------------------------------------------------------
FROM python:3.12-slim AS runtime

ARG ENABLE_OCR=true
ENV PIP_NO_CACHE_DIR=1 \
    PIP_RETRIES=10 \
    PIP_TIMEOUT=60 \
    PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1 \
    MODEL_DIR=/models

# tesseract + the language packs the bot's four locales need. Skipped entirely
# when ENABLE_OCR=false to keep the image ~100 MB smaller.
#
# The retry loop is not paranoia: this pulls ~40 MB of language data, and over a
# VPN or a throttled link apt reliably gets a truncated response part-way
# through. Without it a transient drop fails the whole build.
RUN if [ "${ENABLE_OCR}" = "true" ]; then \
      printf 'Acquire::Retries "10";\nAcquire::http::Timeout "60";\nAcquire::https::Timeout "60";\n' \
        > /etc/apt/apt.conf.d/99-retries \
      && for attempt in 1 2 3 4 5; do \
           apt-get update -qq \
           && apt-get install -y --no-install-recommends --fix-missing \
                tesseract-ocr tesseract-ocr-eng tesseract-ocr-fas \
                tesseract-ocr-ara tesseract-ocr-rus \
           && break; \
           echo "apt attempt ${attempt} failed, retrying"; sleep 10; \
         done \
      && dpkg -s tesseract-ocr > /dev/null \
      && rm -rf /var/lib/apt/lists/*; \
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

"""Detector API tests.

The real ONNX model is a build artefact, so these tests stub `NsfwModel` with
a deterministic fake. That keeps the suite runnable with just
`pip install -r requirements-dev.txt` while still exercising the batching,
per-item error isolation and score-plumbing logic that actually breaks.
"""

from __future__ import annotations

import base64
import io

import numpy as np
import pytest
from fastapi.testclient import TestClient
from PIL import Image

from app import config, main
from app.model import Metadata, _softmax


class FakeModel:
    """Scores an image by its mean red channel — deterministic and cheap."""

    meta = Metadata(
        model_id="fake/test-model",
        labels=["NSFW", "SFW"],
        input_size=[3, 8, 8],
        mean=[0.5, 0.5, 0.5],
        std=[0.5, 0.5, 0.5],
        interpolation="bicubic",
    )

    def preprocess(self, raw: bytes) -> np.ndarray:
        with Image.open(io.BytesIO(raw)) as img:
            arr = np.asarray(img.convert("RGB").resize((8, 8)), dtype=np.float32) / 255.0
        return np.transpose(arr, (2, 0, 1))

    def classify_batch(self, tensors: list[np.ndarray]) -> list[tuple[float, float]]:
        out = []
        for tensor in tensors:
            nsfw = float(tensor[0].mean())
            out.append((nsfw, 1.0 - nsfw))
        return out


def png_b64(color: tuple[int, int, int], size: tuple[int, int] = (32, 32)) -> str:
    buf = io.BytesIO()
    Image.new("RGB", size, color).save(buf, format="PNG")
    return base64.b64encode(buf.getvalue()).decode()


@pytest.fixture
def client(monkeypatch):
    monkeypatch.setattr(main, "_model", FakeModel())
    # TestClient triggers lifespan, which would rebuild the real model; the
    # app object is used directly instead.
    return TestClient(main.app, raise_server_exceptions=True)


def test_health_reports_loaded(client):
    body = client.get("/health").json()
    assert body["status"] == "ok"
    assert body["model_loaded"] is True


def test_model_info_exposes_labels(client):
    body = client.get("/model").json()
    assert body["labels"] == ["NSFW", "SFW"]
    assert body["input_size"] == [3, 8, 8]


def test_classify_returns_score_per_image_in_order(client):
    payload = {
        "images": [
            {"id": "red", "data": png_b64((255, 0, 0))},
            {"id": "black", "data": png_b64((0, 0, 0))},
        ]
    }
    results = client.post("/classify", json=payload).json()["results"]

    assert [r["id"] for r in results] == ["red", "black"]
    assert results[0]["nsfw"] > results[1]["nsfw"]
    for r in results:
        assert 0.0 <= r["nsfw"] <= 1.0
        assert r["error"] is None


def test_one_bad_image_does_not_fail_the_batch(client):
    payload = {
        "images": [
            {"id": "good", "data": png_b64((10, 10, 10))},
            {"id": "garbage", "data": base64.b64encode(b"not an image").decode()},
        ]
    }
    results = {r["id"]: r for r in client.post("/classify", json=payload).json()["results"]}

    assert results["good"]["error"] is None
    assert results["garbage"]["error"] is not None
    assert results["garbage"]["nsfw"] == 0.0


def test_empty_batch_is_accepted(client):
    body = client.post("/classify", json={"images": []}).json()
    assert body["results"] == []


def test_oversized_batch_is_rejected(client):
    images = [{"id": str(i), "data": png_b64((1, 1, 1))} for i in range(config.MAX_BATCH + 1)]
    assert client.post("/classify", json={"images": images}).status_code == 413


def test_ocr_returns_empty_results_when_disabled(client, monkeypatch):
    monkeypatch.setattr(main.ocr, "available", lambda: False)
    body = client.post("/ocr", json={"images": [{"id": "a", "data": png_b64((0, 0, 0))}]}).json()

    assert body["enabled"] is False
    assert body["results"] == [{"id": "a", "text": "", "error": None}]


def test_softmax_rows_sum_to_one():
    probs = _softmax(np.array([[2.0, -1.0], [0.0, 0.0], [100.0, 99.0]]))
    assert np.allclose(probs.sum(axis=-1), 1.0)
    # Large logits must not overflow to nan.
    assert not np.isnan(probs).any()

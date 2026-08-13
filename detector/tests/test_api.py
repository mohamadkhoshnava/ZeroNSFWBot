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
from app.model import Metadata, Score, _softmax


class FakeModel:
    """Scores an image by its mean red channel — deterministic and cheap."""

    meta = Metadata(
        model_id="fake/test-model",
        labels=["NSFW", "SFW"],
        nsfw_labels=["NSFW"],
        input_size=[3, 8, 8],
        mean=[0.5, 0.5, 0.5],
        std=[0.5, 0.5, 0.5],
        interpolation="bicubic",
    )

    def preprocess(self, raw: bytes) -> np.ndarray:
        with Image.open(io.BytesIO(raw)) as img:
            arr = np.asarray(img.convert("RGB").resize((8, 8)), dtype=np.float32) / 255.0
        return np.transpose(arr, (2, 0, 1))

    def classify_batch(self, tensors: list[np.ndarray]) -> list[Score]:
        return [
            Score(nsfw=float(t[0].mean()), labels={"NSFW": float(t[0].mean())})
            for t in tensors
        ]


def png_b64(color: tuple[int, int, int], size: tuple[int, int] = (32, 32)) -> str:
    buf = io.BytesIO()
    Image.new("RGB", size, color).save(buf, format="PNG")
    return base64.b64encode(buf.getvalue()).decode()


class FakeVerifier(FakeModel):
    """Five-class verifier: calls everything a drawing, like a real one would
    for anime art the fast model flags."""

    meta = Metadata(
        model_id="fake/verifier",
        labels=["drawings", "hentai", "neutral", "porn", "sexy"],
        nsfw_labels=["hentai", "porn"],
        input_size=[3, 8, 8],
        mean=[0.5, 0.5, 0.5],
        std=[0.5, 0.5, 0.5],
        interpolation="bicubic",
    )

    def classify_batch(self, tensors: list[np.ndarray]) -> list[Score]:
        return [
            Score(
                nsfw=0.03,
                labels={"drawings": 0.9, "hentai": 0.02, "neutral": 0.06,
                        "porn": 0.01, "sexy": 0.01},
            )
            for _ in tensors
        ]


@pytest.fixture
def client(monkeypatch):
    monkeypatch.setattr(main, "_fast", FakeModel())
    monkeypatch.setattr(main, "_verifier", FakeVerifier())
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


# --------------------------------------------------------------------------
# Second-stage verification.
# --------------------------------------------------------------------------


def test_verify_uses_the_second_model(client):
    body = client.post("/verify", json={"images": [{"id": "a", "data": png_b64((255, 0, 0))}]}).json()

    assert body["available"] is True
    assert body["model_id"] == "fake/verifier"
    result = body["results"][0]
    # The fast model scores this near 1.0; the verifier calls it a drawing.
    assert result["nsfw"] < 0.1
    assert result["labels"]["drawings"] == 0.9


def test_verify_reports_unavailable_when_no_verifier_is_loaded(client, monkeypatch):
    monkeypatch.setattr(main, "_verifier", None)
    body = client.post("/verify", json={"images": [{"id": "a", "data": png_b64((255, 0, 0))}]}).json()

    # Critically: no results and available=false, never the fast model's score.
    assert body["available"] is False
    assert body["results"] == []


def test_health_reports_the_verifier_separately(client):
    body = client.get("/health").json()
    assert body["model_loaded"] is True
    assert body["verifier_loaded"] is True


def test_model_endpoint_describes_both_models(client):
    body = client.get("/model").json()

    assert body["fast"]["labels"] == ["NSFW", "SFW"]
    assert body["verifier"]["labels"] == ["drawings", "hentai", "neutral", "porn", "sexy"]
    # The taxonomy that makes the verifier worth its cost: `drawings` and
    # `sexy` are known classes but deliberately not counted as NSFW.
    assert body["verifier"]["nsfw_labels"] == ["hentai", "porn"]


def test_nsfw_score_sums_only_the_explicit_labels():
    """A multi-class model's NSFW score is the sum of its explicit classes.

    Getting this wrong in either direction is severe: include `drawings` and
    every anime avatar is a ban again; omit `hentai` and explicit anime sails
    through.
    """
    import json as _json
    from pathlib import Path
    from unittest.mock import patch

    import numpy as np

    from app.model import NsfwModel

    meta = {
        "model_id": "m", "labels": ["drawings", "hentai", "neutral", "porn", "sexy"],
        "nsfw_labels": ["hentai", "porn"], "input_size": [3, 8, 8],
        "mean": [0.5] * 3, "std": [0.5] * 3, "interpolation": "bicubic",
    }

    class FakeSession:
        def get_inputs(self):
            return [type("I", (), {"name": "pixel_values"})()]

        def run(self, _out, _feed):
            # drawings .60, hentai .10, neutral .05, porn .20, sexy .05
            return [np.log(np.array([[0.60, 0.10, 0.05, 0.20, 0.05]]))]

    with patch.object(Path, "read_text", lambda self: _json.dumps(meta)), \
         patch.object(NsfwModel, "_load_session", staticmethod(lambda p: FakeSession())):
        model = NsfwModel(Path("/nowhere"))

    score = model.classify_batch([np.zeros((3, 8, 8), dtype=np.float32)])[0]

    assert abs(score.nsfw - 0.30) < 1e-4, "hentai + porn only"
    assert abs(score.labels["drawings"] - 0.60) < 1e-4


def test_a_model_whose_nsfw_labels_are_absent_refuses_to_load():
    """Silently scoring every image 0 would disable moderation entirely."""
    import json as _json
    from pathlib import Path
    from unittest.mock import patch

    from app.model import NsfwModel

    meta = {
        "model_id": "m", "labels": ["cat", "dog"], "nsfw_labels": ["porn"],
        "input_size": [3, 8, 8], "mean": [0.5] * 3, "std": [0.5] * 3,
        "interpolation": "bicubic",
    }

    class FakeSession:
        def get_inputs(self):
            return [type("I", (), {"name": "pixel_values"})()]

    with patch.object(Path, "read_text", lambda self: _json.dumps(meta)), \
         patch.object(NsfwModel, "_load_session", staticmethod(lambda p: FakeSession())):
        with pytest.raises(ValueError, match="every image would score 0"):
            NsfwModel(Path("/nowhere"))

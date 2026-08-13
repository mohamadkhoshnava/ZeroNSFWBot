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


# --------------------------------------------------------------------------
# Frame sampling.
# --------------------------------------------------------------------------


def gif_b64(colors: list[tuple[int, int, int]], size: tuple[int, int] = (32, 32)) -> str:
    """An animated GIF, one solid-colour frame per entry."""
    images = [Image.new("RGB", size, color) for color in colors]
    buf = io.BytesIO()
    images[0].save(buf, format="GIF", save_all=True, append_images=images[1:], duration=100)
    return base64.b64encode(buf.getvalue()).decode()


def test_a_still_image_yields_exactly_one_frame(client):
    body = client.post(
        "/frames", json={"images": [{"id": "a", "data": png_b64((0, 0, 0))}], "max_frames": 5}
    ).json()

    result = body["results"][0]
    assert result["error"] is None
    assert result["total"] == 1
    assert len(result["frames"]) == 1
    assert result["decoder"] == "still"


def test_an_animated_gif_is_sampled_across_its_length(client):
    # 10 frames, sampled 5 times: the sampler must reach the last frame, not
    # just the first — spam GIFs routinely open on something innocuous.
    body = client.post(
        "/frames",
        json={"images": [{"id": "g", "data": gif_b64([(i * 25, 0, 0) for i in range(10)])}],
              "max_frames": 5},
    ).json()

    result = body["results"][0]
    assert result["error"] is None
    assert result["total"] == 10
    assert result["decoder"] == "pillow"

    indices = [f["index"] for f in result["frames"]]
    assert indices[0] == 0
    assert indices[-1] == 9, f"the tail of the clip was never sampled: {indices}"
    assert indices == sorted(indices)
    assert len(indices) == 5


def test_frame_ids_trace_back_to_their_clip(client):
    body = client.post(
        "/frames", json={"images": [{"id": "clip", "data": gif_b64([(0, 0, 0), (9, 9, 9)])}]}
    ).json()

    assert [f["id"] for f in body["results"][0]["frames"]] == ["clip#0", "clip#1"]


def test_extracted_frames_can_be_scored_directly(client):
    """The point of the endpoint: its output is a /classify request body."""
    frames_body = client.post(
        "/frames",
        json={"images": [{"id": "g", "data": gif_b64([(0, 0, 0), (255, 0, 0)])}]},
    ).json()

    images = [{"id": f["id"], "data": f["data"]} for f in frames_body["results"][0]["frames"]]
    results = client.post("/classify", json={"images": images}).json()["results"]

    assert [r["id"] for r in results] == ["g#0", "g#1"]
    # The red frame is the one that scores, and it is not the first — exactly
    # the case a thumbnail-only check misses.
    assert results[1]["nsfw"] > results[0]["nsfw"]


def test_one_undecodable_clip_does_not_fail_the_batch(client):
    body = client.post(
        "/frames",
        json={"images": [
            {"id": "good", "data": png_b64((0, 0, 0))},
            {"id": "junk", "data": base64.b64encode(b"neither image nor video").decode()},
        ]},
    ).json()

    results = {r["id"]: r for r in body["results"]}
    assert results["good"]["error"] is None
    assert results["junk"]["error"] is not None
    assert results["junk"]["frames"] == []


def test_video_without_ffmpeg_is_reported_not_guessed(client, monkeypatch):
    """A silent single-frame fallback would look like a completed check."""
    monkeypatch.setattr(main.frames, "video_available", lambda: False)
    # An MP4 header is enough: the sniffer never gets as far as decoding.
    mp4 = base64.b64encode(b"\x00\x00\x00\x20ftypisom" + b"\x00" * 64).decode()

    body = client.post("/frames", json={"images": [{"id": "v", "data": mp4}]}).json()

    assert body["video_enabled"] is False
    assert "unavailable" in body["results"][0]["error"]


def test_the_frame_limit_is_capped_by_configuration(client):
    body = client.post(
        "/frames",
        json={"images": [{"id": "g", "data": gif_b64([(i, 0, 0) for i in range(60)])}],
              "max_frames": 10_000},
    ).json()

    assert len(body["results"][0]["frames"]) <= config.MAX_FRAMES


@pytest.mark.skipif(
    not __import__("shutil").which("ffmpeg") or not __import__("shutil").which("ffprobe"),
    reason="ffmpeg is not installed on this host",
)
def test_a_real_clip_is_sampled_past_its_opening(tmp_path):
    """The case the whole endpoint exists for.

    Telegram re-encodes uploaded GIFs to MP4 and hands out a poster thumbnail
    taken from the start. A clip that is innocuous for its first half and
    explicit for its second must be caught, which means frames from both.
    """
    import shutil
    import subprocess

    from app import frames as frames_mod

    clip = tmp_path / "clip.mp4"
    subprocess.run(
        [shutil.which("ffmpeg"), "-v", "error", "-y",
         "-f", "lavfi", "-i", "color=c=black:s=64x64:d=2",
         "-f", "lavfi", "-i", "color=c=red:s=64x64:d=2",
         "-filter_complex", "[0:v][1:v]concat=n=2:v=1",
         "-pix_fmt", "yuv420p", str(clip)],
        check=True,
        capture_output=True,
    )

    sampled = frames_mod.sample(clip.read_bytes(), 5)

    assert sampled.decoder == "ffmpeg"
    assert len(sampled.frames) == 5

    reds = [
        Image.open(io.BytesIO(f.data)).convert("RGB").resize((1, 1)).getpixel((0, 0))[0]
        for f in sampled.frames
    ]
    assert reds[0] < 32, "the opening frame should still be the black half"
    assert reds[-1] > 200, "the second half of the clip was never sampled"


def test_the_sniffer_tells_containers_apart():
    from app import frames as frames_mod

    assert frames_mod.sniff(b"\x00\x00\x00\x20ftypisom") == "video"
    assert frames_mod.sniff(b"\x1a\x45\xdf\xa3rest-of-webm") == "video"
    assert frames_mod.sniff(b"GIF89a...") == "image"
    assert frames_mod.sniff(b"\x89PNG\r\n\x1a\n") == "image"


def test_the_spread_covers_both_ends():
    from app.frames import _spread

    assert _spread(1, 5) == [0]
    assert _spread(3, 5) == [0, 1, 2]
    assert _spread(10, 5) == [0, 2, 4, 7, 9]
    assert _spread(100, 3) == [0, 50, 99]


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

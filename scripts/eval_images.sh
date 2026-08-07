#!/usr/bin/env bash
# Score every image in assets/test_images against a running detector and, when
# both folders are populated, report accuracy at each candidate threshold.
#
# This is how you choose the number for /nsfw -> Threshold with evidence rather
# than by guessing.
#
#   docker compose up -d detector
#   ./scripts/eval_images.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DETECTOR_URL="${DETECTOR_URL:-http://localhost:${DETECTOR_PORT:-8000}}"
IMAGES_DIR="${IMAGES_DIR:-$ROOT/assets/test_images}"

command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 1; }

if ! curl -fsS --max-time 5 "$DETECTOR_URL/health" >/dev/null 2>&1; then
  cat >&2 <<EOF
Cannot reach the detector at $DETECTOR_URL

Start it with:
  docker compose up -d detector

Then re-run this script. Override the address with DETECTOR_URL=... if the
service is published somewhere else.
EOF
  exit 1
fi

exec python3 - "$DETECTOR_URL" "$IMAGES_DIR" <<'PYTHON'
import base64
import json
import pathlib
import sys
import urllib.error
import urllib.request

DETECTOR_URL, IMAGES_DIR = sys.argv[1], pathlib.Path(sys.argv[2])
EXTENSIONS = {".png", ".jpg", ".jpeg", ".webp", ".bmp", ".gif"}
BATCH = 8


def post(endpoint: str, payload: dict) -> dict:
    request = urllib.request.Request(
        f"{DETECTOR_URL}{endpoint}",
        data=json.dumps(payload).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=120) as response:
        return json.loads(response.read())


def collect(folder: pathlib.Path) -> list[pathlib.Path]:
    if not folder.is_dir():
        return []
    return sorted(p for p in folder.iterdir() if p.suffix.lower() in EXTENSIONS)


def score(paths: list[pathlib.Path]) -> dict[str, float]:
    scores: dict[str, float] = {}
    for start in range(0, len(paths), BATCH):
        chunk = paths[start : start + BATCH]
        images = [
            {"id": p.name, "data": base64.b64encode(p.read_bytes()).decode()}
            for p in chunk
        ]
        for result in post("/classify", {"images": images})["results"]:
            if result.get("error"):
                print(f"  ! {result['id']}: {result['error']}")
            else:
                scores[result["id"]] = result["nsfw"]
    return scores


model = json.loads(urllib.request.urlopen(f"{DETECTOR_URL}/model", timeout=30).read())
print(f"model: {model['model_id']}  labels={model['labels']}\n")

sfw_files = collect(IMAGES_DIR / "sfw")
nsfw_files = collect(IMAGES_DIR / "nsfw")

if not sfw_files and not nsfw_files:
    print(f"No images found under {IMAGES_DIR}.")
    print("Run `make gen-images` for the synthetic SFW samples, and see")
    print(f"{IMAGES_DIR}/README.md for how to supply your own positives.")
    raise SystemExit(0)

sfw_scores = score(sfw_files)
nsfw_scores = score(nsfw_files)

for label, scores in (("SFW", sfw_scores), ("NSFW", nsfw_scores)):
    if not scores:
        continue
    print(f"{label} ({len(scores)} image(s))")
    for name, value in sorted(scores.items(), key=lambda kv: -kv[1]):
        bar = "#" * round(value * 30)
        print(f"  {value * 100:5.1f}%  {bar:<30}  {name}")
    print()

if not nsfw_scores:
    print("No labelled positives in nsfw/, so recall cannot be measured.")
    print(f"See {IMAGES_DIR}/README.md — the folder is gitignored by design.")
    raise SystemExit(0)

print(f"{'thresh':>7} {'accuracy':>9} {'false pos':>10} {'false neg':>10}")
print("-" * 40)
best = None
for threshold in range(10, 100, 5):
    cut = threshold / 100
    false_pos = sum(1 for v in sfw_scores.values() if v >= cut)
    false_neg = sum(1 for v in nsfw_scores.values() if v < cut)
    total = len(sfw_scores) + len(nsfw_scores)
    accuracy = (total - false_pos - false_neg) / total
    marker = ""
    if best is None or accuracy > best[1]:
        best, marker = (threshold, accuracy), ""
    print(f"{threshold:>6}% {accuracy * 100:>8.1f}% {false_pos:>10} {false_neg:>10}{marker}")

print(f"\nBest accuracy at {best[0]}% ({best[1] * 100:.1f}%).")
print("Set it per group with /nsfw -> Threshold.")
PYTHON

#!/usr/bin/env python3
"""Download the NSFW classifier and export it to ONNX.

Runs once, at image build time, inside a throwaway stage that has torch and
timm installed. The runtime image only ships the resulting ``model.onnx`` plus
``metadata.json``, so onnxruntime is the only inference dependency there.

``metadata.json`` makes the runtime self-describing: input size, normalisation
constants and label order all come from the model's own config rather than
being hardcoded in two places.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path

import timm
import torch

# The model card for Marqo/nsfw-image-detection-384 documents this order and
# timm does not carry label names in its config, so it is the fallback.
FALLBACK_LABELS = ["NSFW", "SFW"]


def resolve_labels(model: torch.nn.Module, num_classes: int) -> list[str]:
    """Best-effort recovery of class names from whatever the config exposes."""
    cfg = getattr(model, "pretrained_cfg", {}) or {}
    for key in ("label_names", "labels", "class_names"):
        labels = cfg.get(key)
        if labels and len(labels) == num_classes:
            return [str(label) for label in labels]

    override = os.environ.get("DETECTOR_LABELS")
    if override:
        labels = [part.strip() for part in override.split(",") if part.strip()]
        if len(labels) == num_classes:
            return labels

    if num_classes == len(FALLBACK_LABELS):
        return list(FALLBACK_LABELS)

    return [f"class_{i}" for i in range(num_classes)]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--model-id",
        default=os.environ.get("MODEL_ID", "Marqo/nsfw-image-detection-384"),
        help="Hugging Face hub id of a timm-compatible classifier.",
    )
    parser.add_argument("--out-dir", default="/models", type=Path)
    parser.add_argument("--opset", type=int, default=17)
    args = parser.parse_args()

    args.out_dir.mkdir(parents=True, exist_ok=True)

    print(f"[export] loading {args.model_id} ...", flush=True)
    model = timm.create_model(f"hf_hub:{args.model_id}", pretrained=True)
    model.eval()

    data_config = timm.data.resolve_data_config({}, model=model)
    channels, height, width = data_config["input_size"]
    num_classes = int(getattr(model, "num_classes", 0)) or int(
        model.get_classifier().out_features
    )
    labels = resolve_labels(model, num_classes)

    print(
        f"[export] input={channels}x{height}x{width} "
        f"classes={num_classes} labels={labels}",
        flush=True,
    )

    dummy = torch.randn(1, channels, height, width)
    onnx_path = args.out_dir / "model.onnx"

    torch.onnx.export(
        model,
        dummy,
        str(onnx_path),
        input_names=["pixel_values"],
        output_names=["logits"],
        # Batch must be dynamic: the bot classifies a user's profile photos
        # (and any message media) in a single request.
        dynamic_axes={"pixel_values": {0: "batch"}, "logits": {0: "batch"}},
        opset_version=args.opset,
        do_constant_folding=True,
    )

    metadata = {
        "model_id": args.model_id,
        "labels": labels,
        "input_size": [channels, height, width],
        "mean": list(data_config["mean"]),
        "std": list(data_config["std"]),
        "interpolation": data_config.get("interpolation", "bicubic"),
        "crop_pct": data_config.get("crop_pct", 1.0),
        "opset": args.opset,
    }
    (args.out_dir / "metadata.json").write_text(json.dumps(metadata, indent=2))

    size_mb = onnx_path.stat().st_size / (1024 * 1024)
    print(f"[export] wrote {onnx_path} ({size_mb:.1f} MB)", flush=True)


if __name__ == "__main__":
    main()

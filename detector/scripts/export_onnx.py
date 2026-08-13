#!/usr/bin/env python3
"""Download a classifier and export it to ONNX.

Runs at image build time, inside a throwaway stage that has torch, timm and
transformers installed. The runtime image only ships the resulting
``model.onnx`` plus ``metadata.json``, so onnxruntime is the only inference
dependency there.

Two loaders are supported because the two models the detector uses come from
different ecosystems: the fast screening model is a timm checkpoint, the
verifier is a transformers ``ViTForImageClassification``. Which one to use is
detected automatically unless ``--loader`` says otherwise.

``metadata.json`` makes the runtime self-describing: input size, normalisation
constants, label order and which labels count as NSFW all come from the model's
own config rather than being hardcoded in two places.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path

import torch

# Label names that mean "explicit" across the taxonomies we support. Note what
# is *absent*: `drawings` and `sexy`. Separating an ordinary anime portrait from
# actual pornography is the entire reason the verifier exists, so folding those
# buckets back in would defeat it.
DEFAULT_NSFW_LABELS = ("nsfw", "porn", "hentai")

# timm's config carries no label names and the Marqo card documents this order.
TIMM_FALLBACK_LABELS = ["NSFW", "SFW"]


def resolve_nsfw_labels(labels: list[str]) -> list[str]:
    """Which of `labels` count towards the NSFW probability."""
    override = os.environ.get("NSFW_LABELS")
    if override:
        wanted = {p.strip().lower() for p in override.split(",") if p.strip()}
    else:
        wanted = set(DEFAULT_NSFW_LABELS)

    matched = [label for label in labels if label.lower() in wanted]
    if matched:
        return matched

    # Nothing matched by name — fall back to "everything that is not obviously
    # safe", which is right for binary models whose classes are named oddly.
    safe = {"sfw", "normal", "neutral", "safe", "drawings"}
    return [label for label in labels if label.lower() not in safe]


def load_timm(model_id: str):
    import timm

    model = timm.create_model(f"hf_hub:{model_id}", pretrained=True)
    model.eval()

    cfg = timm.data.resolve_data_config({}, model=model)
    num_classes = int(getattr(model, "num_classes", 0)) or int(
        model.get_classifier().out_features
    )

    labels = None
    for key in ("label_names", "labels", "class_names"):
        candidate = (getattr(model, "pretrained_cfg", {}) or {}).get(key)
        if candidate and len(candidate) == num_classes:
            labels = [str(x) for x in candidate]
            break
    if labels is None:
        labels = (
            list(TIMM_FALLBACK_LABELS)
            if num_classes == len(TIMM_FALLBACK_LABELS)
            else [f"class_{i}" for i in range(num_classes)]
        )

    return model, {
        "input_size": list(cfg["input_size"]),
        "mean": list(cfg["mean"]),
        "std": list(cfg["std"]),
        "interpolation": cfg.get("interpolation", "bicubic"),
        "labels": labels,
    }


def preprocessing_for(model_id: str, config) -> dict:
    """The three numbers the runtime needs: input size, mean and std.

    Tried in descending order of trustworthiness, because model repos vary in
    how well-formed they are and getting normalisation wrong silently shifts
    every score:

    1. ``AutoImageProcessor`` — correct whenever the repo is current.
    2. The raw ``preprocessor_config.json``. Repos written for older
       transformers name their processor ``ViTFeatureExtractor``, a class v5
       removed, so Auto refuses them even though the actual values are right
       there in the file.
    3. The model config's ``image_size`` with ViT's default 0.5/0.5
       normalisation.
    """
    try:
        from transformers import AutoImageProcessor

        processor = AutoImageProcessor.from_pretrained(model_id)
        size = processor.size
        edge = size.get("height") or size.get("shortest_edge") or 224
        return {
            "input_size": [3, int(edge), int(edge)],
            "mean": [float(x) for x in processor.image_mean],
            "std": [float(x) for x in processor.image_std],
        }
    except Exception as exc:  # noqa: BLE001
        print(f"[export] AutoImageProcessor declined ({exc}); reading the raw config", flush=True)

    try:
        from huggingface_hub import hf_hub_download

        raw = json.loads(
            Path(hf_hub_download(model_id, "preprocessor_config.json")).read_text()
        )
        size = raw.get("size") or {}
        edge = size.get("height") or size.get("shortest_edge") or 224
        return {
            "input_size": [3, int(edge), int(edge)],
            "mean": [float(x) for x in raw["image_mean"]],
            "std": [float(x) for x in raw["image_std"]],
        }
    except Exception as exc:  # noqa: BLE001
        print(f"[export] no usable preprocessor config ({exc}); using ViT defaults", flush=True)

    edge = int(getattr(config, "image_size", 224))
    return {"input_size": [3, edge, edge], "mean": [0.5] * 3, "std": [0.5] * 3}


def load_transformers(model_id: str):
    from transformers import AutoModelForImageClassification

    model = AutoModelForImageClassification.from_pretrained(model_id)
    model.eval()

    # id2label keys arrive as ints or strings depending on how the config was
    # written; sorting numerically keeps the order matching the logits.
    id2label = model.config.id2label
    labels = [str(id2label[key]) for key in sorted(id2label, key=lambda k: int(k))]

    return model, {
        **preprocessing_for(model_id, model.config),
        "interpolation": "bicubic",
        "labels": labels,
    }


class LogitsOnly(torch.nn.Module):
    """Unwrap a transformers model so ONNX sees a plain logits tensor.

    `ViTForImageClassification` returns an `ImageClassifierOutput` dataclass;
    exporting that produces a graph whose output name depends on the transformers
    version. The runtime reads output 0 and nothing else, so pin it here.
    """

    def __init__(self, model: torch.nn.Module) -> None:
        super().__init__()
        self.model = model

    def forward(self, pixel_values: torch.Tensor) -> torch.Tensor:
        return self.model(pixel_values=pixel_values).logits


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--model-id",
        default=os.environ.get("MODEL_ID", "Marqo/nsfw-image-detection-384"),
        help="Hugging Face hub id of an image classifier.",
    )
    parser.add_argument("--out-dir", default="/models", type=Path)
    parser.add_argument("--opset", type=int, default=17)
    parser.add_argument(
        "--loader",
        choices=("auto", "timm", "transformers"),
        default="auto",
    )
    parser.add_argument(
        "--no-dynamo",
        dest="dynamo",
        action="store_false",
        help="Use the legacy TorchScript exporter. An escape hatch for when a "
        "torch release breaks the dynamo path; the resulting graph is scored "
        "by the CI smoke test either way.",
    )
    args = parser.parse_args()

    args.out_dir.mkdir(parents=True, exist_ok=True)

    print(f"[export] loading {args.model_id} (loader={args.loader}) ...", flush=True)

    if args.loader == "timm":
        model, cfg = load_timm(args.model_id)
    elif args.loader == "transformers":
        model, cfg = load_transformers(args.model_id)
        model = LogitsOnly(model)
    else:
        try:
            model, cfg = load_timm(args.model_id)
        except Exception as exc:  # noqa: BLE001 - any timm failure means "try the other one"
            print(f"[export] timm could not load it ({exc}); trying transformers", flush=True)
            model, cfg = load_transformers(args.model_id)
            model = LogitsOnly(model)

    channels, height, width = cfg["input_size"]
    nsfw_labels = resolve_nsfw_labels(cfg["labels"])

    print(
        f"[export] input={channels}x{height}x{width} labels={cfg['labels']} "
        f"nsfw={nsfw_labels}",
        flush=True,
    )
    if not nsfw_labels:
        raise SystemExit(
            f"none of {cfg['labels']} count as NSFW — set NSFW_LABELS explicitly, "
            "or the model will score everything 0"
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
        # Stated explicitly rather than left to the default, which flipped from
        # the TorchScript tracer to dynamo in torch 2.6 and changed what the
        # export needs installed. Pinning it here means a torch bump can only
        # fail loudly at build time, never quietly produce a different graph.
        dynamo=args.dynamo,
    )

    metadata = {
        "model_id": args.model_id,
        "labels": cfg["labels"],
        "nsfw_labels": nsfw_labels,
        "input_size": cfg["input_size"],
        "mean": cfg["mean"],
        "std": cfg["std"],
        "interpolation": cfg["interpolation"],
        "opset": args.opset,
    }
    (args.out_dir / "metadata.json").write_text(json.dumps(metadata, indent=2))

    size_mb = onnx_path.stat().st_size / (1024 * 1024)
    print(f"[export] wrote {onnx_path} ({size_mb:.1f} MB)", flush=True)


if __name__ == "__main__":
    main()

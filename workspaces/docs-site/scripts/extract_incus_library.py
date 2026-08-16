#!/usr/bin/env python3
"""Extract the archived Incus contact sheets into normalized UI sprites.

The source sheets are JPEG exports whose visible checkerboard is baked into the
pixels.  This script therefore uses a conservative colour matte, fills enclosed
light regions (mugs, paper, and similar props), trims the result, and writes
small alpha WebP assets for the docs runtime.
"""

from __future__ import annotations

import argparse
import json
import shutil
from collections import deque
from dataclasses import dataclass
from pathlib import Path

from PIL import Image, ImageFilter


@dataclass(frozen=True)
class Sprite:
    sheet: str
    panel: int
    category: str
    name: str
    crop_bottom: int = 853
    include: bool = True
    reject_reason: str = ""


SPRITES = (
    Sprite("a01", 0, "tip", "pointing", 690),
    Sprite("a01", 1, "info", "open-palm", 690),
    Sprite("a01", 2, "warning", "visor-alert", 690),
    Sprite("a01", 3, "neutral", "portrait", 690),
    Sprite("a02", 0, "tip", "lightbulb", 690),
    Sprite("a02", 1, "info", "rubber-duck", 690),
    Sprite("a02", 2, "easter-egg", "rtfm-coffee", 690),
    Sprite("a03", 0, "easter-egg", "soda", 690),
    Sprite("a03", 1, "easter-egg", "animal-mug", 690),
    Sprite("a03", 2, "javascript", "javascript-coffee", 690),
    Sprite("a04", 0, "python", "python-book"),
    Sprite("a04", 1, "python", "python-snake"),
    Sprite("a04", 2, "python", "python-hat"),
    Sprite("a05", 0, "rust", "rust-shield"),
    Sprite("a05", 1, "rust", "rust-crab"),
    Sprite("a05", 2, "rust", "rust-mug"),
    Sprite("a06", 0, "python", "python-duck"),
    Sprite("a06", 1, "python", "python-laptop"),
    Sprite("a06", 2, "python", "python-noodles"),
    Sprite("a07", 0, "hint", "magnifying-glass"),
    Sprite("a07", 1, "hint", "treasure-map"),
    Sprite("a07", 2, "hint", "checklist"),
    Sprite("a08", 0, "hint", "lightbulb"),
    Sprite("a08", 1, "hint", "question"),
    Sprite("a08", 2, "hint", "next-arrow"),
    Sprite("a09", 0, "composed-failure", "usb-mismatch"),
    Sprite("a09", 1, "warning", "sparking-cables"),
    Sprite("a09", 2, "warning", "danger-switch"),
    Sprite("a10", 0, "easter-egg", "shrug"),
    Sprite("a10", 1, "easter-egg", "doge", include=False, reject_reason="embedded generated text"),
    Sprite("a10", 2, "easter-egg", "zero-problems", include=False, reject_reason="embedded generated text"),
    Sprite("b01", 0, "easter-egg", "flower"),
    Sprite("b01", 1, "info", "dna"),
    Sprite("b01", 2, "easter-egg", "energy-sword"),
    Sprite("b02", 0, "easter-egg", "goldfish"),
    Sprite("b02", 1, "neutral", "sleepy"),
    Sprite("b02", 2, "easter-egg", "pool-duck"),
    Sprite("b03", 0, "easter-egg", "goldfish-alt"),
    Sprite("b03", 1, "neutral", "music"),
    Sprite("b03", 2, "easter-egg", "bubbles"),
    Sprite("b04", 0, "easter-egg", "adventurer"),
    Sprite("b04", 1, "easter-egg", "wizard", include=False, reject_reason="pale beard cannot be cleanly recovered from JPEG checkerboard"),
    Sprite("b04", 2, "seasonal-october", "zombie"),
    Sprite("b05", 0, "system", "data-cube"),
    Sprite("b05", 1, "system", "connect-nodes"),
    Sprite("b05", 2, "system", "orchestrate"),
    Sprite("b06", 0, "neutral", "standing"),
    Sprite("b06", 1, "neutral", "three-quarter"),
    Sprite("b06", 2, "neutral", "hands-behind-back"),
    Sprite("b07", 0, "composed-failure", "cracked-cube"),
    Sprite("b07", 1, "composed-failure", "leaking-pipe"),
    Sprite("b07", 2, "composed-failure", "broken-connector"),
    Sprite("b08", 0, "success", "flex"),
    Sprite("b08", 1, "success", "arms-crossed"),
    Sprite("b08", 2, "success", "dumbbell"),
    Sprite("b09", 0, "info", "equation-book"),
    Sprite("b09", 1, "easter-egg", "laptop-coffee"),
    Sprite("b09", 2, "success", "calculator"),
    Sprite("b10", 0, "success", "arms-up"),
    Sprite("b10", 1, "success", "cube-coffee"),
    Sprite("b10", 2, "success", "trophy"),
)


def panel_bounds(width: int, count: int, panel: int) -> tuple[int, int]:
    left = round(width * panel / count)
    right = round(width * (panel + 1) / count)
    return left + 2, right - 2


def matte_alpha(image: Image.Image) -> Image.Image:
    rgb = image.convert("RGB")
    width, height = rgb.size
    pixels = list(rgb.get_flattened_data())
    raw: list[int] = []

    for red, green, blue in pixels:
        maximum = max(red, green, blue)
        minimum = min(red, green, blue)
        saturation = (maximum - minimum) / maximum if maximum else 0.0
        luminance = 0.2126 * red + 0.7152 * green + 0.0722 * blue
        # The JPEG checkerboard is sometimes tinted by the generated glow.
        # Only strong chroma or genuinely dark material seeds foreground; pale
        # props are recovered by the enclosed-hole pass below.
        saturation_score = max(0.0, min(1.0, (saturation - 0.34) / 0.18))
        darkness_score = max(0.0, min(1.0, (127.0 - luminance) / 62.0))
        alpha = int(255 * max(saturation_score, darkness_score))
        raw.append(0 if alpha < 28 else min(255, int((alpha - 28) * 1.15)))

    # Fill transparent-looking holes that are enclosed by foreground. This
    # recovers neutral props without reintroducing the checkerboard outside.
    transparent = [value < 34 for value in raw]
    exterior = bytearray(width * height)
    queue: deque[int] = deque()

    def seed(index: int) -> None:
        if transparent[index] and not exterior[index]:
            exterior[index] = 1
            queue.append(index)

    for x in range(width):
        seed(x)
        seed((height - 1) * width + x)
    for y in range(height):
        seed(y * width)
        seed(y * width + width - 1)

    while queue:
        index = queue.popleft()
        x = index % width
        y = index // width
        if x:
            neighbour = index - 1
            if transparent[neighbour] and not exterior[neighbour]:
                exterior[neighbour] = 1
                queue.append(neighbour)
        if x + 1 < width:
            neighbour = index + 1
            if transparent[neighbour] and not exterior[neighbour]:
                exterior[neighbour] = 1
                queue.append(neighbour)
        if y:
            neighbour = index - width
            if transparent[neighbour] and not exterior[neighbour]:
                exterior[neighbour] = 1
                queue.append(neighbour)
        if y + 1 < height:
            neighbour = index + width
            if transparent[neighbour] and not exterior[neighbour]:
                exterior[neighbour] = 1
                queue.append(neighbour)

    for index, is_transparent in enumerate(transparent):
        if is_transparent and not exterior[index]:
            raw[index] = 255

    alpha = Image.new("L", (width, height))
    alpha.putdata(raw)
    alpha = alpha.filter(ImageFilter.MaxFilter(3)).filter(ImageFilter.GaussianBlur(0.7))

    # The source art intentionally dissolves into particles. Reinforce that
    # dissolve so JPEG residue never creates a hard lower edge.
    alpha_pixels = list(alpha.get_flattened_data())
    fade_start = int(height * 0.82)
    for y in range(fade_start, height):
        factor = max(0.0, (height - 1 - y) / max(1, height - fade_start))
        for x in range(width):
            index = y * width + x
            alpha_pixels[index] = int(alpha_pixels[index] * factor)
    alpha.putdata(alpha_pixels)
    return alpha


def normalize(image: Image.Image, alpha: Image.Image) -> Image.Image:
    rgba = image.convert("RGBA")
    rgba.putalpha(alpha)
    bbox = alpha.getbbox()
    if bbox is None:
        raise ValueError("empty matte")

    left, top, right, bottom = bbox
    padding = 10
    left = max(0, left - padding)
    top = max(0, top - padding)
    right = min(rgba.width, right + padding)
    bottom = min(rgba.height, bottom + padding)
    trimmed = rgba.crop((left, top, right, bottom))

    canvas_width, canvas_height = 480, 640
    scale = min((canvas_width - 16) / trimmed.width, (canvas_height - 16) / trimmed.height, 1.0)
    target = (max(1, round(trimmed.width * scale)), max(1, round(trimmed.height * scale)))
    if target != trimmed.size:
        trimmed = trimmed.resize(target, Image.Resampling.LANCZOS)

    canvas = Image.new("RGBA", (canvas_width, canvas_height), (0, 0, 0, 0))
    x = (canvas_width - trimmed.width) // 2
    y = canvas_height - trimmed.height
    canvas.alpha_composite(trimmed, (x, y))
    return canvas


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source-a", type=Path, required=True)
    parser.add_argument("--source-b", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    sources: dict[str, Path] = {}
    for prefix, directory in (("a", args.source_a), ("b", args.source_b)):
        for number in range(1, 11):
            sources[f"{prefix}{number:02}"] = directory / f"{number}-Photo-{number}.jpg"

    args.output.mkdir(parents=True, exist_ok=True)
    for category in {sprite.category for sprite in SPRITES}:
        shutil.rmtree(args.output / category, ignore_errors=True)
    for source in sources.values():
        if not source.exists():
            raise FileNotFoundError(source)

    manifest: dict[str, list[str]] = {}
    records: list[dict[str, object]] = []

    for sprite in SPRITES:
        if not sprite.include:
            records.append({
                "source": sprite.sheet,
                "panel": sprite.panel,
                "category": sprite.category,
                "name": sprite.name,
                "included": False,
                "reason": sprite.reject_reason,
            })
            continue
        source = Image.open(sources[sprite.sheet]).convert("RGB")
        panel_count = 4 if sprite.sheet == "a01" else 3
        left, right = panel_bounds(source.width, panel_count, sprite.panel)
        crop = source.crop((left, 0, right, min(sprite.crop_bottom, source.height)))
        alpha = matte_alpha(crop)
        normalized = normalize(crop, alpha)

        category_dir = args.output / sprite.category
        category_dir.mkdir(exist_ok=True)
        filename = f"incus_{sprite.category.replace('-', '_')}_{sprite.name.replace('-', '_')}.webp"
        destination = category_dir / filename
        normalized.save(destination, "WEBP", quality=88, method=6, exact=True)
        relative = f"/shared/incapunk/incus-library/{sprite.category}/{filename}"
        manifest.setdefault(sprite.category, []).append(relative)
        records.append({
            "source": sprite.sheet,
            "panel": sprite.panel,
            "category": sprite.category,
            "name": sprite.name,
            "path": relative,
            "included": True,
        })

    (args.output / "manifest.json").write_text(
        json.dumps({"pools": manifest, "sprites": records}, indent=2) + "\n",
        encoding="utf-8",
    )
    manifest_js = "window.INCAN_INCUS_POOLS = Object.freeze(" + json.dumps(manifest, indent=2) + ");\n"
    (args.output / "manifest.js").write_text(manifest_js, encoding="utf-8")


if __name__ == "__main__":
    main()

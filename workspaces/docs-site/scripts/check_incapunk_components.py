#!/usr/bin/env python3
"""Validate the durable contracts behind the Incapunk docs components."""

from __future__ import annotations

import json
import re
from pathlib import Path


DOCS_SITE = Path(__file__).resolve().parents[1]
DOCS = DOCS_SITE / "docs"
THEME = DOCS / "shared/incapunk/incapunk.css"
RUNTIME = DOCS / "_snippets/javascripts/incan_docs_components.js"
HOME_MOTION = DOCS / "_snippets/javascripts/incan_home_motion.js"
MANIFEST = DOCS / "shared/incapunk/incus-library/manifest.json"
MANIFEST_JS = DOCS / "shared/incapunk/incus-library/manifest.js"
MKDOCS = DOCS_SITE / "mkdocs.yml"

REQUIRED_POOLS = {
    "tip",
    "hint",
    "info",
    "warning",
    "neutral",
    "easter-egg",
    "javascript",
    "python",
    "rust",
    "composed-failure",
    "seasonal-october",
    "system",
    "success",
}

REQUIRED_CSS_HOOKS = {
    ".inc-route-grid",
    ".inc-step-rail",
    ".inc-learning-panel",
    ".inc-book-progress",
    ".inc-prev-next",
    ".inc-release-summary",
    ".inc-incus-slot--active",
    ".inc-oven-route",
    ".inc-oven-proof-story",
    ".inc-oven-mechanic",
    ".inc-oven-evidence",
    ".rfc-index-filter",
}


REPRESENTATIVE_PAGES = {
    "start_here/index.md": ("inc-route-grid",),
    "start_here/coming_from_python.md": (
        "inc-bridge-note",
        'data-incus-category="python"',
    ),
    "start_here/coming_from_rust.md": (
        "inc-bridge-note",
        'data-incus-category="rust"',
    ),
    "language/tutorials/book/index.md": (
        "inc-book-progress",
        "inc-chapter-grid",
    ),
    "language/tutorials/book/01_hello_world.md": (
        "inc-book-progress",
        "inc-learning-panel--exercise",
        "inc-learning-panel--solution",
        "inc-learning-panel--complete",
        "inc-prev-next",
    ),
    "tooling/tutorials/getting_started.md": (
        "inc-step-rail",
        "inc-learning-panel--result",
        "inc-learning-panel--complete",
    ),
    "language/tutorials/build_your_first_api.md": (
        "inc-step-rail",
        "inc-learning-panel--result",
        "inc-learning-panel--complete",
    ),
    "release_notes/0_4.md": (
        "inc-release-summary",
        "inc-learning-panel--warning",
    ),
    "RFCs/index.md": ("inc-route-grid",),
    "project/incus.md": ('id="meet-incus-title"',),
    "tooling/explanation/oven_alpha.md": (
        "inc-oven-route__grid",
        "inc-oven-capabilities",
        "inc-oven-proof-story",
        "inc-oven-mechanic--commands",
        "inc-oven-evidence__grid",
        'id="current-alpha-boundary"',
    ),
}


def asset_path(site_path: str) -> Path:
    return DOCS / site_path.removeprefix("/")


def main() -> int:
    errors: list[str] = []

    def require(condition: bool, message: str) -> None:
        if not condition:
            errors.append(message)

    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    pools = manifest.get("pools", {})
    sprites = manifest.get("sprites", [])

    missing_pools = REQUIRED_POOLS - pools.keys()
    require(not missing_pools, f"missing semantic Incus pools: {sorted(missing_pools)}")
    for category in REQUIRED_POOLS:
        require(bool(pools.get(category)), f"Incus pool {category!r} is empty")

    all_pool_paths: list[str] = []
    for category, paths in pools.items():
        require(isinstance(paths, list), f"Incus pool {category!r} is not a list")
        for path in paths:
            all_pool_paths.append(path)
            require(asset_path(path).is_file(), f"missing Incus asset: {path}")

    require(
        len(all_pool_paths) == len(set(all_pool_paths)),
        "an Incus asset appears in more than one semantic pool",
    )

    included_paths = {
        sprite["path"]
        for sprite in sprites
        if sprite.get("included") and "path" in sprite
    }
    require(
        included_paths == set(all_pool_paths),
        "manifest sprite records and semantic pools contain different included assets",
    )

    seasonal_paths = set(pools.get("seasonal-october", []))
    require(bool(seasonal_paths), "seasonal-october pool is empty")
    require(
        all("zombie" in path for path in seasonal_paths),
        "seasonal-october contains a non-zombie asset",
    )
    ordinary_paths = {
        path
        for category, paths in pools.items()
        if category != "seasonal-october"
        for path in paths
    }
    require(
        not seasonal_paths & ordinary_paths,
        "October-only assets also appear in an ordinary pool",
    )

    manifest_js = MANIFEST_JS.read_text(encoding="utf-8")
    expected_manifest_js = (
        "window.INCAN_INCUS_POOLS = Object.freeze("
        f"{json.dumps(pools, indent=2, ensure_ascii=False)}"
        ");\n"
    )
    require(
        manifest_js == expected_manifest_js,
        "manifest.js is not the exact generated serialization of manifest.json pools",
    )

    runtime = RUNTIME.read_text(encoding="utf-8")
    require('image.alt = "";' in runtime, "Incus images must have empty alternative text")
    require(
        'image.setAttribute("aria-hidden", "true");' in runtime,
        "Incus images must be hidden from assistive technology",
    )
    require(
        bool(
            re.search(
                r"const selected = slots\[[^\]]+\];[\s\S]+?"
                r"selected\.appendChild\(image\);",
                runtime,
            )
        )
        and runtime.count("appendChild(image)") == 1,
        "runtime must have exactly one selected-slot Incus append path",
    )
    require(
        "new Date().getMonth() === 9" in runtime,
        "October-only selection is missing or uses the wrong month",
    )
    require(
        '["tip", "hint", "info", "neutral"].includes(category)' in runtime,
        "playful Easter eggs must remain limited to low-stakes Incus slots",
    )
    require(
        'hash(`${window.location.pathname}:easter-egg`) % 7 === 0' in runtime,
        "low-stakes Incus slots no longer have their deterministic one-in-seven Easter egg",
    )
    require(
        'version.textContent = "dev docs";' in runtime
        and '/(^|\\/)dev(\\/|$)/' in runtime,
        "development deployments no longer replace the stable release badge with a docs-channel identity",
    )

    home_motion = HOME_MOTION.read_text(encoding="utf-8")
    require(
        'document.querySelector(".inc-hero__flow")' in home_motion
        and 'hero.querySelector(".inc-hero__flow")' not in home_motion,
        "homepage compiler flow must be resolved outside the hero container",
    )

    css = THEME.read_text(encoding="utf-8")
    require("!important" not in css, "incapunk.css contains !important")
    require(
        bool(
            re.search(
                r"\.md-typeset \.inc-incus-slot__image\s*\{[^}]*"
                r"pointer-events:\s*none;",
                css,
            )
        ),
        "Incus image selector must not capture pointer events",
    )
    require(
        bool(
            re.search(
                r"\.md-typeset \.inc-incus-slot--active\s*\{[^}]*"
                r"padding-right:\s*calc\(var\(--incus-slot-width\) "
                r"\+ var\(--incus-slot-gap\)\);",
                css,
            )
        ),
        "active Incus slots do not reserve horizontal content clearance",
    )
    require(
        "@keyframes incan-wordmark-sheen" in css
        and "@keyframes incan-wordmark-glow" in css
        and "@keyframes incan-wordmark-arrive" in css
        and '.inc-hero__brand::before' in css
        and 'mask: url("wordmark_small_001.png")' in css,
        "homepage wordmark no longer has its arrival, glow, and masked sheen animations",
    )
    for hook in REQUIRED_CSS_HOOKS:
        require(hook in css, f"missing reusable CSS hook: {hook}")

    mkdocs = MKDOCS.read_text(encoding="utf-8")
    require("scheme: slate" in mkdocs, "the supported docs palette must remain dark")
    require("scheme: default" not in mkdocs, "the unsupported light palette was reintroduced")
    manifest_position = mkdocs.find("shared/incapunk/incus-library/manifest.js")
    runtime_position = mkdocs.find("_snippets/javascripts/incan_docs_components.js")
    require(
        0 <= manifest_position < runtime_position,
        "Incus manifest.js must load before the component runtime",
    )
    project_block = mkdocs.split("  - Project:", maxsplit=1)[-1].strip()
    require(
        project_block.endswith("- Meet Incus: project/incus.md"),
        "Meet Incus must be the final Project navigation entry",
    )

    homepage = (DOCS / "index.md").read_text(encoding="utf-8")
    require('href="project/incus/"' in homepage, "homepage observer does not link to Meet Incus")
    require(
        'aria-label="Meet Incus"' in homepage,
        "homepage observer link has no accessible name",
    )

    oven_page = (DOCS / "tooling/explanation/oven_alpha.md").read_text(
        encoding="utf-8"
    )
    require(
        'class="inc-oven-route__grid" markdown="1"' not in oven_page,
        "Oven route cards must remain direct grid children rather than Markdown paragraphs",
    )
    require(
        '<ol class="inc-oven-proof-story"' in oven_page
        and oven_page.count("<li><span>") == 3,
        "Oven publication story must expose publisher, seal, and consumer stages",
    )
    require(
        ".inc-motion-capable .inc-hero__flow .inc-flow-step" in css
        and ".inc-motion-capable .inc-hero .inc-flow-step" not in css,
        "reduced-motion handling must target the compiler flow outside the hero",
    )

    for relative_path, markers in REPRESENTATIVE_PAGES.items():
        page = (DOCS / relative_path).read_text(encoding="utf-8")
        for marker in markers:
            require(marker in page, f"{relative_path} is missing component marker {marker!r}")

    rfc_table = (DOCS / "_snippets/tables/rfcs_index.md").read_text(encoding="utf-8")
    require(
        "rfc-index-filter" in rfc_table and "data-rfc-filter" in rfc_table,
        "generated RFC index snippet is missing its filter controls",
    )

    if errors:
        print("Incapunk component contract failed:")
        for error in errors:
            print(f"- {error}")
        return 1

    print(
        "Incapunk component contract passed "
        f"({len(all_pool_paths)} assets, {len(REPRESENTATIVE_PAGES)} representative pages)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

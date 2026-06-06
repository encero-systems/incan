#!/usr/bin/env python3
"""Render the Homebrew formula from an SDK release manifest."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


TARGETS = {
    "aarch64-apple-darwin": ("macos", "arm"),
    "x86_64-apple-darwin": ("macos", "intel"),
    "x86_64-unknown-linux-gnu": ("linux", "x86_64"),
}


def ruby_string(value: str) -> str:
    return value.replace("\\", "\\\\").replace('"', '\\"')


def host_block(target: str, host: dict[str, str], indent: str) -> str:
    url = ruby_string(host["archive_url"])
    checksum = ruby_string(host["archive_sha256"])
    return f'{indent}url "{url}"\n{indent}sha256 "{checksum}"'


def render_formula(manifest: dict[str, object]) -> str:
    hosts = manifest.get("hosts", {})
    if not isinstance(hosts, dict):
        raise ValueError("manifest hosts must be an object")

    missing = sorted(target for target in TARGETS if target not in hosts)
    if missing:
        raise ValueError(f"manifest is missing Homebrew targets: {', '.join(missing)}")

    version = ruby_string(str(manifest["sdk_version"]))
    mac_arm = host_block("aarch64-apple-darwin", hosts["aarch64-apple-darwin"], "      ")
    mac_intel = host_block("x86_64-apple-darwin", hosts["x86_64-apple-darwin"], "      ")
    linux_x86 = host_block("x86_64-unknown-linux-gnu", hosts["x86_64-unknown-linux-gnu"], "      ")

    return f'''# typed: false
# frozen_string_literal: true

class Incan < Formula
  desc "Statically typed language that compiles to native Rust"
  homepage "https://github.com/dannys-code-corner/incan"
  version "{version}"

  on_macos do
    if Hardware::CPU.arm?
{mac_arm}
    else
{mac_intel}
    end
  end

  on_linux do
{linux_x86}
  end

  def install
    bin.install "bin/incan"
    bin.install "bin/incan-lsp"
  end

  test do
    assert_match version.to_s, shell_output("#{{bin}}/incan --version")
  end
end
'''


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()

    manifest = json.loads(args.manifest.read_text(encoding="utf-8"))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(render_formula(manifest), encoding="utf-8")


if __name__ == "__main__":
    main()

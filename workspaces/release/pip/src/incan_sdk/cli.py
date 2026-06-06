"""Command shims for the Incan SDK Python package."""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path


def _package_root() -> Path:
    return Path(__file__).resolve().parents[2]


def _installer_script() -> Path:
    candidates = [
        Path(__file__).resolve().parent / "vendor" / "install-incan-sdk.sh",
        _package_root().parent / "install-incan-sdk.sh",
    ]
    for candidate in candidates:
        if candidate.exists():
            return candidate
    raise RuntimeError("could not find bundled install-incan-sdk.sh")


def _sdk_home() -> Path:
    return Path(os.environ.get("INCAN_PIP_SDK_HOME", _package_root() / ".incan" / "home"))


def _bin_dir() -> Path:
    return Path(os.environ.get("INCAN_PIP_BIN_DIR", _package_root() / ".incan" / "bin"))


def _has_value_option(args: list[str], name: str) -> bool:
    return name in args or any(arg.startswith(f"{name}=") for arg in args)


def _installer_args(args: list[str]) -> list[str]:
    next_args = list(args)
    if not _has_value_option(next_args, "--incan-home"):
        next_args.extend(["--incan-home", str(_sdk_home())])
    if not _has_value_option(next_args, "--bin-dir"):
        next_args.extend(["--bin-dir", str(_bin_dir())])
    return next_args


def _run_installer(args: list[str]) -> int:
    return subprocess.call(["bash", str(_installer_script()), *_installer_args(args)])


def install() -> None:
    raise SystemExit(_run_installer(sys.argv[1:]))


def _command_path(command: str) -> Path:
    return _bin_dir() / command


def _ensure_command(command: str) -> None:
    if _command_path(command).exists():
        return
    status = _run_installer([])
    if status != 0:
        raise SystemExit(status)


def _run_command(command: str) -> None:
    _ensure_command(command)
    os.execv(str(_command_path(command)), [command, *sys.argv[1:]])


def incan() -> None:
    _run_command("incan")


def incan_lsp() -> None:
    _run_command("incan-lsp")


if __name__ == "__main__":
    if len(sys.argv) >= 2 and sys.argv[1] == "install":
        sys.argv.pop(1)
        install()
    if len(sys.argv) >= 2 and sys.argv[1] == "incan-lsp":
        sys.argv.pop(1)
        incan_lsp()
    incan()

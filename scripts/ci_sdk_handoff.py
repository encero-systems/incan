"""Transfer one compiler-selected SDK between jobs without a cache or publisher fallback.

The envelope binds transport bytes and build coordinates. The compiler remains the authority for
SDK identity and inventory compatibility; consuming runs its canary with an explicit inventory.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import sys
import tempfile
import time


DEADLINE = time.monotonic() + 270


def run(command, workspace, environment):
    """Bound external tools and preserve their diagnostics on failure."""
    result = subprocess.run(command, cwd=workspace, env=environment, capture_output=True,
                            timeout=max(1, DEADLINE - time.monotonic()))
    if result.returncode:
        details = (result.stderr + result.stdout).decode(errors="replace").strip()
        raise ValueError(f"command failed ({result.returncode}): {details}")
    return result.stdout


def file_digest(path):
    """Hash potentially large compiler binaries without loading them into memory."""
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def payload_digest(root):
    """Bind relative paths and bytes, refusing links outside the transferred tree."""
    digest = hashlib.sha256()
    for path in sorted(root.rglob("*")):
        if path.is_symlink():
            raise ValueError(f"SDK handoff cannot contain a symlink: {path}")
        name = path.relative_to(root).as_posix().encode()
        digest.update(len(name).to_bytes(8, "big"))
        digest.update(name)
        if path.is_dir():
            digest.update(b"directory\0")
        elif path.is_file():
            digest.update(b"file\0")
            digest.update(bytes.fromhex(file_digest(path)))
        else:
            raise ValueError(f"unsupported SDK handoff entry: {path}")
    return digest.hexdigest()


def coordinates(options):
    """Derive expected inputs independently of the downloaded envelope."""
    environment = dict(os.environ, INCAN_NO_BANNER="1", INCAN_SOURCE_ROOT=str(options.workspace))
    identity = run([str(options.compiler), "oven", "sdk-provider-store-identity", "--compiler-root",
                    str(options.workspace)], options.workspace, environment).decode().strip()
    if len(identity) != 64 or any(char not in "0123456789abcdef" for char in identity):
        raise ValueError("compiler returned an invalid SDK provider identity")
    rustc = run([str(options.rustc), "--version", "--verbose"], options.workspace, environment)
    return {"schema_version": 1, "provider_identity": identity,
            "compiler_sha256": file_digest(options.compiler), "rustc_identity": hashlib.sha256(rustc).hexdigest()}


def inventory(root):
    """Require the exact published inventory before any compiler command can prepare providers."""
    path = root / "sdk-inventory.json"
    if not path.is_file() or path.is_symlink():
        raise ValueError(f"SDK inventory is missing or not a regular file: {path}")
    return path


def stage(options, expected):
    """Package only the successfully prewarmed identity; never copy unrelated store entries."""
    selected = options.store / expected["provider_identity"]
    inventory(selected)
    if selected.is_symlink():
        raise ValueError(f"SDK handoff cannot contain a symlink: {selected}")
    expected["payload_sha256"] = payload_digest(selected)
    # GitHub's file-oriented artifact upload omits empty directories, but provider integrity includes them.
    expected["empty_directories"] = [path.relative_to(selected).as_posix() for path in sorted(selected.rglob("*"))
                                     if path.is_dir() and not any(path.iterdir())]
    if options.artifact.exists():
        raise ValueError(f"SDK handoff output already exists: {options.artifact}")
    options.artifact.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="sdk-handoff-", dir=options.artifact.parent) as temporary:
        staged = Path(temporary) / "artifact"
        staged.mkdir()
        copied = staged / expected["provider_identity"]
        shutil.copytree(selected, copied)
        if payload_digest(copied) != expected["payload_sha256"]:
            raise ValueError("SDK payload changed during handoff publication")
        (staged / "handoff.json").write_text(json.dumps(expected, indent=2) + "\n")
        staged.rename(options.artifact)
    print(f"Staged SDK handoff {expected['provider_identity']} ({expected['payload_sha256']})")


def consume(options, expected):
    """Validate transport and compiler compatibility before exporting any consumer paths."""
    envelope = json.loads((options.artifact / "handoff.json").read_text())
    if not isinstance(envelope, dict) or envelope.get("schema_version") != 1:
        raise ValueError("unsupported handoff schema")
    for key, label in [("provider_identity", "provider identity"), ("compiler_sha256", "compiler digest"),
                       ("rustc_identity", "rustc identity")]:
        if envelope.get(key) != expected[key]:
            raise ValueError(f"SDK handoff {label} mismatch")
    selected = options.artifact / expected["provider_identity"]
    selected_inventory = inventory(selected)
    if selected.is_symlink():
        raise ValueError(f"SDK handoff cannot contain a symlink: {selected}")
    directories = envelope.get("empty_directories")
    if not isinstance(directories, list):
        raise ValueError("SDK handoff is missing its empty-directory manifest")
    for directory in directories:
        if not isinstance(directory, str):
            raise ValueError("invalid SDK directory manifest entry")
        relative = PurePosixPath(directory)
        if (not directory or relative.is_absolute() or ".." in relative.parts
                or relative.as_posix() != directory or directory == "."):
            raise ValueError("SDK directory manifest entry must remain inside the selected provider")
        path = selected / directory
        if not path.resolve().is_relative_to(selected.resolve()):
            raise ValueError("SDK directory manifest entry escapes the selected provider")
        path.mkdir(parents=True, exist_ok=True)
    if payload_digest(selected) != envelope.get("payload_sha256"):
        raise ValueError("SDK handoff payload digest mismatch")
    environment = dict(os.environ, INCAN_NO_BANNER="1", CARGO_NET_OFFLINE="true",
                       INCAN_SOURCE_ROOT=str(options.workspace), INCAN_SDK_INVENTORY=str(selected_inventory))
    # Explicit inventory discovery rejects absent/incompatible data before source-checkout preparation is considered.
    try:
        output = run([str(options.compiler), "check", "tests/fixtures/test_assert_canary.incn"],
                     options.workspace, environment)
    except ValueError as error:
        raise ValueError(f"SDK validation failed: {error}") from error
    if payload_digest(selected) != envelope["payload_sha256"]:
        raise ValueError("SDK validation modified the transferred payload")
    variables = {"INCAN_SDK_INVENTORY": str(selected_inventory),
                 "INCAN_INTERNAL_SDK_PROVIDER_STORE": str(options.artifact),
                 "INCAN_INTERNAL_SDK_PROVIDER_PATH_FILE": str(options.path_file),
                 "INCAN_TEST_SDK_PROVIDER_STORE": str(options.artifact),
                 "INCAN_TEST_SDK_PROVIDER_PATH_FILE": str(options.path_file)}
    if any("\n" in value or "\r" in value for value in variables.values()):
        raise ValueError("SDK handoff paths cannot contain line breaks")
    options.path_file.parent.mkdir(parents=True, exist_ok=True)
    options.path_file.write_text(f"{selected}\n")
    with options.env_file.open("a") as environment_file:
        environment_file.writelines(f"{key}={value}\n" for key, value in variables.items())
    sys.stdout.buffer.write(output)
    print(f"Validated SDK handoff {expected['provider_identity']}; source publication was not requested")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["stage", "consume"])
    parser.add_argument("--workspace", type=Path, default=Path.cwd())
    parser.add_argument("--compiler", type=Path, default=Path("target/debug/incan"))
    parser.add_argument("--rustc", default="rustc")
    parser.add_argument("--artifact", type=Path, required=True)
    parser.add_argument("--store", type=Path)
    parser.add_argument("--path-file", type=Path)
    parser.add_argument("--env-file", type=Path)
    options = parser.parse_args()
    for name in ("workspace", "compiler", "artifact", "store", "path_file", "env_file"):
        path = getattr(options, name)
        if path is not None:
            setattr(options, name, path.resolve())
    if options.mode == "stage" and options.store is None:
        parser.error("stage requires --store")
    if options.mode == "consume" and (options.path_file is None or options.env_file is None):
        parser.error("consume requires --path-file and --env-file")
    try:
        expected = coordinates(options)
        (stage if options.mode == "stage" else consume)(options, expected)
    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
        print(f"SDK handoff failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())

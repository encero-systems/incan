"""Describe Linux bootstrap tool inputs and verify their output-only transport.

This is a candidate identity, not proof of complete compiler input coverage. Workflow cache
activation requires separately audited transitive depfiles, build-script inputs and environment.
"""

import hashlib
import json
from pathlib import Path, PurePosixPath
import stat
import subprocess


TOOLS = ("incan", "generate_lang_reference", "generate_feature_inventory")
ROOT_INPUTS = {"Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "rust-toolchain"}


def digest(path):
    """Hash a regular file without accepting symlink substitution."""
    if path.is_symlink() or not path.is_file():
        raise ValueError(f"expected a regular file: {path}")
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def canonical_digest(value):
    """Bind a JSON value independently of insignificant serialization whitespace."""
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def covered_path(name):
    """Select conservative compiler source and configuration, not workflow/test administration."""
    path = PurePosixPath(name)
    return (name in ROOT_INPUTS or path.parts[0] in ("src", "crates", "assets", ".cargo")
            or name == "scripts/ci_tool_outputs.py")


def input_manifest(workspace, coordinates):
    """Bind tracked source bytes and explicit external build coordinates to this checkout.

The caller must supply the actual recipe, rustc/cargo/linker identities, runner image and effective
build environment. Empty coordinate records are refused rather than inventing defaults. The
absolute checkout remains an input while compiler code embeds CARGO_MANIFEST_DIR.
    """
    required = {"recipe", "rustc", "cargo", "linker", "runner_image", "environment"}
    if set(coordinates) != required or any(not coordinates[key] for key in required - {"environment"}):
        raise ValueError("incomplete or unexpected build coordinates")
    if not isinstance(coordinates["environment"], dict):
        raise ValueError("environment coordinates must be an explicit mapping")
    workspace = workspace.resolve(strict=True)
    result = subprocess.run(["git", "ls-files", "-z"], cwd=workspace, check=True,
                            capture_output=True, timeout=30)
    tracked = set(result.stdout.decode().split("\0"))
    for name in (".cargo/config", ".cargo/config.toml"):
        if (workspace / name).exists() and name not in tracked:
            raise ValueError(f"untracked Cargo configuration: {name}")
    records = []
    for name in sorted(tracked):
        if not name or not covered_path(name):
            continue
        path = workspace / name
        if path.resolve() != path.absolute():
            raise ValueError(f"input contains a symlink: {name}")
        records.append({"path": name, "mode": stat.S_IMODE(path.stat().st_mode), "sha256": digest(path)})
    if not {"Cargo.toml", "Cargo.lock"}.issubset({record["path"] for record in records}):
        raise ValueError("tracked Cargo manifest and lock are required")
    inputs = {"schema_version": 1, "workspace": str(workspace),
              "coordinates": coordinates, "files": records}
    return {"identity": canonical_digest(inputs), "inputs": inputs}


def verify_local_inputs(manifest, consumed_paths):
    """Refuse any local consumed path not covered by the candidate's unchanged file record.

Feed only independently classified local inputs here. Registry/toolchain inputs and generated
OUT_DIR files need their own provenance validation before a complete build can be admitted.
    """
    verifier = ConsumedInputs(manifest)
    if not consumed_paths:
        raise ValueError("no consumed local input evidence")
    for value in consumed_paths:
        path = Path(value)
        path = path if path.is_absolute() else verifier.workspace / path
        verifier.local_record(path)


def output_manifest(directory, expected_identity):
    """Describe exactly the three regular executable outputs for the expected input identity."""
    if directory.is_symlink():
        raise ValueError("tool directory cannot be a symlink")
    outputs = {}
    for name in TOOLS:
        path = directory / name
        value = digest(path)
        if not path.stat().st_mode & 0o111:
            raise ValueError(f"tool is not executable: {name}")
        outputs[name] = value
    return {"schema_version": 1, "input_identity": expected_identity, "outputs": outputs}


def verify_outputs(directory, manifest, expected_identity):
    """Require the independently expected identity and all three actual output hashes.

The caller treats ValueError as a cache miss, builds normally, and never stages rejected bytes.
This function alone does not assert that the bundle's source-input coverage was admitted.
    """
    actual = output_manifest(directory, expected_identity)
    if manifest != actual:
        raise ValueError("tool bundle identity, membership or output digest mismatch")

# Keep the build recipe in the input identity and the workflow in agreement. The workflow contract
# tests additionally check that cache verification precedes either conditional build command.
RECIPE = [
    "CARGO_BUILD_JOBS=2 cargo build --locked --release --features lsp --bin incan --bin generate_feature_inventory --message-format=json-render-diagnostics",
    "CARGO_BUILD_JOBS=2 cargo build --locked --release -p incan_core --bin generate_lang_reference --message-format=json-render-diagnostics",
]
BUILD_ENVIRONMENT = ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUSTDOCFLAGS", "CC", "CXX", "AR",
                     "CFLAGS", "CXXFLAGS", "LDFLAGS", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER",
                     "CARGO_BUILD_TARGET", "CARGO_BUILD_RUSTFLAGS", "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER",
                     "CARGO_PROFILE_RELEASE_OPT_LEVEL", "CARGO_PROFILE_RELEASE_LTO",
                     "CARGO_PROFILE_RELEASE_CODEGEN_UNITS", "CARGO_PROFILE_RELEASE_DEBUG",
                     "CARGO_PROFILE_RELEASE_STRIP", "CARGO_PROFILE_RELEASE_PANIC", "CARGO_INCREMENTAL")


# Acquisition/diagnostic settings cannot become published values or native compile-input identity.
ACQUISITION_ENVIRONMENT = {"CARGO_TERM_COLOR", "CARGO_TERM_VERBOSE", "CARGO_TERM_QUIET",
                           "CARGO_NET_OFFLINE", "CARGO_NET_RETRY", "CARGO_NET_GIT_FETCH_WITH_CLI",
                           "RUSTUP_DIST_SERVER", "RUSTUP_UPDATE_ROOT", "RUST_BACKTRACE", "RUST_LIB_BACKTRACE",
                           "RUST_LOG", "RUSTUP_IO_THREADS", "RUSTUP_MAX_RETRIES"}
BUILD_ENVIRONMENT += ("CARGO_HOME", "CARGO_TARGET_DIR", "CARGO_BUILD_JOBS", "RUSTUP_HOME",
                      "RUSTUP_TOOLCHAIN", "PATH", "LD_LIBRARY_PATH", "LIBRARY_PATH", "CPATH")


def acquisition_environment(name):
    """Identify credential or transport settings without inspecting or retaining their values."""
    return (name in ACQUISITION_ENVIRONMENT or name.startswith("CARGO_HTTP_")
            or name == "CARGO_REGISTRY_TOKEN"
            or (name.startswith("CARGO_REGISTRIES_") and name.endswith("_TOKEN")))


def build_environment(environment):
    """Capture only reviewed compilation coordinates; unknown configuration refuses eligibility.

Credentials and network acquisition settings never enter the key, state or diagnostic text.
Only unsupported variable names are reported. Executed extensions remain separately unadmitted,
so their arbitrary access to excluded environment cannot silently become an accepted dependency.
    """
    unsupported = sorted(name for name in environment if name.startswith(("RUST", "CARGO"))
                         and name not in BUILD_ENVIRONMENT and not acquisition_environment(name))
    if unsupported:
        raise ValueError("unsupported build configuration names: " + ", ".join(unsupported))
    return {name: environment.get(name) for name in sorted(BUILD_ENVIRONMENT)}


def command_text(command, workspace):
    """Read a bounded tool identity without compiling or acquiring dependencies."""
    return subprocess.run(command, cwd=workspace, check=True, capture_output=True,
                          text=True, timeout=30).stdout.strip()


def depfile_facts(path):
    """Read rustc's concrete Make-style source paths and explicit env-dep annotations.

Unsupported variable syntax refuses admission. This parser does not infer dependency edges or
build-script inputs; Cargo's artifact messages choose the depfiles to inspect.
    """
    import shlex
    text = path.read_text().replace("\\\n", "")
    inputs = set()
    environment = {}
    for line in text.splitlines():
        if line.startswith("# env-dep:"):
            name, separator, value = line[len("# env-dep:"):].partition("=")
            if not name or name in environment:
                raise ValueError(f"ambiguous env-dep in {path}")
            environment[name] = value if separator else None
        elif line and not line.startswith("#"):
            if "$" in line or ": " not in line:
                # rustc also emits empty phony targets; these carry no input evidence.
                if line.endswith(":") and "$" not in line:
                    continue
                raise ValueError(f"unsupported depfile syntax in {path}")
            inputs.update(shlex.split(line.split(": ", 1)[1], comments=False))
    if not inputs:
        raise ValueError(f"depfile contains no inputs: {path}")
    return sorted(inputs), environment


def artifact_depfiles(message, workspace):
    """Locate depfiles only for Cargo-reported concrete output filenames."""
    result = set()
    for name in message.get("filenames", []):
        output = Path(name)
        if message.get("target", {}).get("kind") == ["custom-build"]:
            import re
            import shlex
            package = message.get("package_id", "").rsplit("#", 1)[-1].split("@", 1)[0]
            match = re.fullmatch(re.escape(package) + r"-([0-9a-f]{16})", output.parent.name)
            if output.name != "build-script-build" or not match:
                raise ValueError("custom-build output has no exact package/fingerprint owner")
            candidate = output.with_name("build_script_build-" + match[1] + ".d")
            candidate = normalize_owned_path(candidate, output.parent)
            sources, _ = depfile_facts(candidate)
            expected_source = str(Path(message["target"]["src_path"]))
            targets = set()
            for line in candidate.read_text().replace("\\\n", "").splitlines():
                if line and not line.startswith("#") and ": " in line:
                    targets.update(shlex.split(line.split(": ", 1)[0]))
            if expected_source not in sources or str(candidate.with_suffix("")) not in targets:
                raise ValueError("custom-build depfile does not bind its reported source and output")
            result.add(candidate)
            continue
        stem = output.stem
        if stem.startswith("lib") and output.suffix in (".rlib", ".rmeta", ".so", ".dylib", ".a"):
            stem = stem[3:]
        candidate = output.with_name(stem + ".d")
        if candidate.is_file():
            result.add(candidate)
    executable = message.get("executable")
    if executable:
        candidate = Path(executable).with_suffix(".d")
        if candidate.is_file():
            result.add(candidate)
    if not result:
        raise ValueError(f"no concrete depfile for Cargo artifact {message.get('package_id')}")
    return sorted(result)


def verify_cargo_environment(message, name, value, verifier):
    """Check common Cargo-injected constants against the concrete reported package manifest."""
    manifest = Path(message["manifest_path"])
    package = verifier.toml(manifest)["package"]
    if name == "CARGO_MANIFEST_DIR":
        expected = str(manifest.parent)
    elif name in ("CARGO_PKG_NAME", "CARGO_PKG_VERSION"):
        field = "name" if name.endswith("NAME") else "version"
        expected = package[field]
        if isinstance(expected, dict) and expected == {"workspace": True}:
            expected = verifier.toml(verifier.workspace / "Cargo.toml")["workspace"]["package"][field]
    else:
        raise ValueError(f"Cargo constant needs explicit projection audit: {name}")
    if value != expected:
        raise ValueError(f"Cargo constant differs from reported package: {name}")


def normalize_owned_path(path, owner):
    """Normalize compiler include paths without traversing links or leaving their original owner."""
    path, owner = Path(path), Path(owner)
    if ".." in owner.parts or not path.is_absolute() or not path.is_relative_to(owner):
        raise ValueError("consumed path has no original source owner")
    current = Path(path.anchor)
    for part in owner.parts[1:]:
        current /= part
        if current.is_symlink():
            raise ValueError("consumed source owner contains a symlink")
    for part in path.relative_to(owner).parts:
        if part == "..":
            if not current.is_dir():
                raise ValueError("consumed path traverses a non-directory")
            if current == owner:
                raise ValueError("consumed path escapes its source owner")
            current = current.parent
        elif part != ".":
            current /= part
            if current.is_symlink():
                raise ValueError("consumed input contains a symlink")
    if not current.is_file():
        raise ValueError("consumed input is not a regular file")
    return current


class ConsumedInputs:
    """Reuse parsed immutable package evidence within one admission or verification operation."""

    def __init__(self, candidate):
        import os
        self.workspace = Path(candidate["inputs"]["workspace"])
        self.records = {record["path"]: record for record in candidate["inputs"]["files"]}
        self.registry = Path(os.environ.get("CARGO_HOME", str(Path.home() / ".cargo"))) / "registry/src"
        self.manifests = {}
        self.registry_packages = {}
        self.locked = None

    def toml(self, path):
        """Parse each selected manifest or lock once, retaining its complete value."""
        import tomllib
        if path not in self.manifests:
            self.manifests[path] = tomllib.loads(path.read_text())
        return self.manifests[path]

    def local_record(self, path):
        """Check each local file's bytes and mode once and return that verified digest."""
        path = normalize_owned_path(path, self.workspace)
        try:
            name = path.relative_to(self.workspace).as_posix()
        except ValueError as error:
            raise ValueError(f"consumed input outside source candidate: {path}") from error
        record = self.records.get(name)
        if record is None:
            raise ValueError(f"uncovered consumed input: {path}")
        value = digest(path)
        if record["sha256"] != value or record["mode"] != stat.S_IMODE(path.stat().st_mode):
            raise ValueError(f"consumed input bytes or mode changed: {path}")
        return {"path": str(path), "sha256": value, "source": "workspace"}

    def record(self, path):
        """Verify local source or a registry file against its cached exact locked checksum owner."""
        if path.is_relative_to(self.workspace):
            return self.local_record(path)
        if not path.is_relative_to(self.registry):
            raise ValueError("input is neither covered workspace source nor a regular locked registry file")
        relative = path.relative_to(self.registry)
        if len(relative.parts) < 3:
            raise ValueError("registry source path has no package owner")
        root = self.registry / relative.parts[0] / relative.parts[1]
        path = normalize_owned_path(path, root)
        if root not in self.registry_packages:
            package = self.toml(root / "Cargo.toml")["package"]
            if root.name != package["name"] + "-" + package["version"]:
                raise ValueError("extracted registry directory differs from its package identity")
            if self.locked is None:
                self.locked = {}
                for entry in self.toml(self.workspace / "Cargo.lock")["package"]:
                    if entry.get("source", "").startswith("registry+"):
                        key = (entry["name"], entry["version"])
                        self.locked.setdefault(key, []).append(entry)
            matches = self.locked.get((package["name"], package["version"]), [])
            if len(matches) != 1 or not matches[0].get("checksum"):
                raise ValueError("registry input lacks one exact locked checksum owner")
            owner = matches[0]
            archive = self.registry.parent / "cache" / relative.parts[0] / (root.name + ".crate")
            files = authenticated_archive_files(archive, owner["checksum"], root.name)
            if files.get("Cargo.toml") != digest(root / "Cargo.toml"):
                raise ValueError("extracted package manifest differs from the locked archive")
            self.registry_packages[root] = (files, owner)
        files, owner = self.registry_packages[root]
        value = digest(path)
        if files.get(path.relative_to(root).as_posix()) != value:
            raise ValueError("registry file differs from the locked archive")
        return {"path": str(path), "sha256": value, "source": owner["source"],
                "package_checksum": owner["checksum"]}


def authenticated_archive_files(archive, checksum, package_directory):
    """Hash regular archive members after authenticating the same open archive against Cargo.lock.

No files are extracted. Duplicate, escaping or linked entries refuse the entire owner, including
unused entries, so later consumed-path lookups cannot select an ambiguous archive interpretation.
    """
    import tarfile
    if archive.is_symlink() or archive.resolve() != archive.absolute() or not archive.is_file():
        raise ValueError("locked registry archive is absent or linked")
    files = {}
    seen = set()
    try:
        with archive.open("rb") as source:
            if hashlib.file_digest(source, "sha256").hexdigest() != checksum:
                raise ValueError("registry archive checksum differs from Cargo.lock")
            source.seek(0)
            with tarfile.open(fileobj=source, mode="r:gz") as package:
                for member in package:
                    parts = member.name.rstrip("/").split("/")
                    if (not parts or parts[0] != package_directory or "\\" in member.name
                            or any(part in ("", ".", "..") for part in parts)):
                        raise ValueError("registry archive contains an escaping or ambiguous path")
                    relative = "/".join(parts[1:])
                    if relative in seen:
                        raise ValueError("registry archive contains duplicate paths")
                    seen.add(relative)
                    if member.isdir():
                        continue
                    if not member.isfile() or not relative:
                        raise ValueError("registry archive contains a linked or unsupported entry")
                    stream = package.extractfile(member)
                    if stream is None:
                        raise ValueError("registry archive member is unreadable")
                    with stream:
                        files[relative] = hashlib.file_digest(stream, "sha256").hexdigest()
    except tarfile.TarError as error:
        raise ValueError("locked registry archive cannot be decoded") from error
    if "Cargo.toml" not in files:
        raise ValueError("registry archive omits its package manifest")
    return files


def collect_evidence(workspace, candidate, logs, environment):
    """Collect actual Cargo/rustc inputs, refusing opaque executed extension code.

Build scripts and proc macros can read arbitrary files or environment without informing Cargo.
Until each concrete extension has an independently reviewed input contract, this collector emits
its identity as an admission refusal. Rerun directives are evidence, not a hermeticity assertion.
    """
    verifier = ConsumedInputs(candidate) if candidate is not None else None
    paths = set()
    env_facts = {}
    artifacts = []
    refusals = [] if candidate is not None else [{"kind": "identity-ineligible-evidence-only"}]
    finished = 0
    tools = set()
    for log in logs:
        for line in log.read_text().splitlines():
            if not line.strip():
                continue
            message = json.loads(line)
            reason = message.get("reason")
            if reason == "build-finished":
                if not message.get("success"):
                    raise ValueError("Cargo reported an unsuccessful build")
                finished += 1
            elif reason == "build-script-executed":
                output = Path(message["out_dir"]).parent / "output"
                directives = output.read_text() if output.is_file() and output.stat().st_size <= 131072 else None
                refusals.append({"kind": "unreviewed-build-script", "package_id": message.get("package_id"),
                                 "out_dir": message.get("out_dir"), "env_names": [name for name, _ in message.get("env", [])],
                                 "linked_path_count": len(message.get("linked_paths", [])),
                                 "linked_paths_sha256": canonical_digest(message.get("linked_paths", [])),
                                 "env_sha256": canonical_digest(message.get("env", [])),
                                 "cargo_directive_names": sorted({line.split("=", 1)[0] for line in directives.splitlines()
                                                                  if line.startswith(("cargo:", "cargo::"))}) if directives else [],
                                 "cargo_output_available": directives is not None,
                                 "cargo_output_sha256": hashlib.sha256(directives.encode()).hexdigest() if directives is not None else None})
            elif reason == "compiler-artifact":
                target = message.get("target", {})
                if "proc-macro" in target.get("kind", []):
                    refusals.append({"kind": "unreviewed-proc-macro", "package_id": message.get("package_id"),
                                     "source": target.get("src_path")})
                if target.get("name") in TOOLS and message.get("executable"):
                    expected = workspace / "target/release" / target["name"]
                    if Path(message["executable"]).resolve() != expected.resolve():
                        raise ValueError("Cargo tool output does not match the release delivery path")
                    tools.add(target["name"])
                artifacts.append({key: message.get(key) for key in
                                  ("package_id", "manifest_path", "target", "profile", "features", "fresh", "filenames")})
                try:
                    for depfile in artifact_depfiles(message, workspace):
                        source_paths, observed_env = depfile_facts(depfile)
                        paths.update(source_paths)
                        for name, value in observed_env.items():
                            # Cargo supplies per-package coordinates; they are not ambient process variables.
                            if name.startswith("CARGO_PKG_") or name == "CARGO_MANIFEST_DIR":
                                try:
                                    if verifier is None:
                                        raise ValueError("package environment is unadmitted without an eligible identity")
                                    verify_cargo_environment(message, name, value, verifier)
                                except (ValueError, OSError, KeyError) as error:
                                    refusals.append({"kind": "cargo-environment-projection-needs-audit", "name": name,
                                                     "detail": str(error)})
                            elif name not in BUILD_ENVIRONMENT or environment.get(name) != value:
                                refusals.append({"kind": "unaccounted-environment", "name": name})
                            else:
                                env_facts[name] = value
                except (ValueError, OSError) as error:
                    refusals.append({"kind": "depfile-unavailable", "detail": str(error)})
    if finished != len(logs) or tools != set(TOOLS):
        raise ValueError("Cargo evidence must finish both recipes and name all three tool outputs")
    records = []
    for name in sorted(paths):
        path = Path(name)
        path = path if path.is_absolute() else workspace / path
        try:
            if verifier is None:
                # This is an observation, never a reusable-input assertion. Refuse following linked inputs.
                import os
                registry = Path(os.environ.get("CARGO_HOME", str(Path.home() / ".cargo"))) / "registry/src"
                if (path.is_relative_to(workspace) or path.is_relative_to(registry)) and path.resolve() == path.absolute():
                    records.append({"path": str(path), "sha256": digest(path), "source": "unadmitted-observation"})
                else:
                    refusals.append({"kind": "unadmitted-external-input", "path": str(path)})
            else:
                records.append(verifier.record(path))
        except (ValueError, OSError) as error:
            refusals.append({"kind": "input-coverage-unavailable", "path": str(path), "detail": str(error)})
    if not records:
        refusals.append({"kind": "no-covered-local-inputs"})
    return {"admitted": not refusals, "refusals": refusals, "files": records,
            "environment": env_facts, "artifacts": artifacts,
            "observed_tool_outputs": {name: digest(workspace / "target/release" / name) for name in TOOLS}}


def verify_bundle(bundle, candidate, environment):
    """Admit a warm bundle only after recomputing source, consumed files and output digests."""
    if bundle.is_symlink() or (bundle / "manifest.json").is_symlink():
        raise ValueError("bundle or manifest cannot be a symlink")
    manifest = json.loads((bundle / "manifest.json").read_text())
    if manifest.get("inputs") != candidate or not manifest.get("evidence", {}).get("admitted"):
        raise ValueError("bundle input identity or admission evidence mismatch")
    evidence = manifest["evidence"]
    if evidence.get("refusals") or not evidence.get("files"):
        raise ValueError("bundle has incomplete consumed-input evidence")
    verifier = ConsumedInputs(candidate)
    for record in evidence["files"]:
        if verifier.record(Path(record["path"])) != record:
            raise ValueError("consumed input bytes changed")
    for name, value in evidence["environment"].items():
        if environment.get(name) != value:
            raise ValueError(f"consumed environment changed: {name}")
    verify_outputs(bundle, manifest["tools"], candidate["identity"])
    return manifest


def main():
    """Run the producer's observable identity, restore verification and cold admission stages."""
    import argparse
    import os
    import shutil
    import time
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("identity", "restore", "admit", "collect"))
    parser.add_argument("--workspace", type=Path, default=Path.cwd())
    parser.add_argument("--state", type=Path, required=True)
    parser.add_argument("--bundle", type=Path, required=True)
    parser.add_argument("--cargo-log", type=Path, action="append", default=[])
    options = parser.parse_args()
    start = time.monotonic()
    workspace = options.workspace.resolve(strict=True)
    state = options.state
    state.mkdir(parents=True, exist_ok=True)
    report = {"operation": options.operation}
    outputs = {}
    if options.operation == "identity":
        workflow = (workspace / ".github/workflows/ci.yml").read_text()
        for command in RECIPE:
            if workflow.count(command + " | python3 scripts/ci_tool_outputs.py record ") != 1:
                raise ValueError("workflow build command differs from the fingerprinted recipe")
        coordinates = {"recipe": RECIPE, "rustc": command_text(["rustc", "-vV"], workspace),
                       "cargo": command_text(["cargo", "-vV"], workspace),
                       "linker": command_text(["cc", "--version"], workspace) + "\n" + command_text(["ld", "--version"], workspace),
                       "runner_image": [os.environ.get("ImageOS"), os.environ.get("ImageVersion")],
                       "environment": build_environment(os.environ)}
        if not all(coordinates["runner_image"]):
            raise ValueError("runner image coordinates are unavailable")
        # Ambient Cargo configuration can change sources, flags and wrappers independently of the repository.
        cargo_home = Path(os.environ.get("CARGO_HOME", str(Path.home() / ".cargo")))
        for root in [cargo_home, *workspace.parents]:
            for name in ("config", "config.toml"):
                path = root / name if root == cargo_home else root / ".cargo" / name
                if path.exists():
                    raise ValueError(f"unadmitted ambient Cargo configuration: {path}")
        candidate = input_manifest(workspace, coordinates)
        (state / "inputs.json").write_text(json.dumps(candidate, indent=2) + "\n")
        outputs["key"] = "incan-linux-tools-v1-" + candidate["identity"]
        outputs["eligible"] = "true"
    elif options.operation == "collect":
        if len(options.cargo_log) != 2:
            raise ValueError("two completed Cargo JSON logs are required")
        evidence = collect_evidence(workspace, None, options.cargo_log, os.environ)
        (state / "coverage.json").write_text(json.dumps(evidence, indent=2) + "\n")
        outputs["admitted"] = "false"
        report.update(status="evidence-only", refusal_count=len(evidence["refusals"]))
    else:
        candidate = json.loads((state / "inputs.json").read_text())
        for name, value in candidate["inputs"]["coordinates"]["environment"].items():
            if os.environ.get(name) != value:
                raise ValueError(f"build environment changed after identity: {name}")
        # Recompute every candidate byte before restore or cold publication.
        if candidate != input_manifest(workspace, candidate["inputs"]["coordinates"]):
            raise ValueError("source inputs changed after identity selection")
        if options.operation == "restore":
            try:
                verify_bundle(options.bundle, candidate, os.environ)
                (workspace / "target/release").mkdir(parents=True, exist_ok=True)
                for name in TOOLS:
                    shutil.copy2(options.bundle / name, workspace / "target/release" / name)
                report["status"] = "hit"
                outputs["hit"] = "true"
            except (OSError, ValueError, KeyError, TypeError) as error:
                report.update(status="miss", detail=str(error))
                outputs["hit"] = "false"
                if options.bundle.exists() or options.bundle.is_symlink():
                    rejected = options.bundle.with_name(options.bundle.name + "-rejected")
                    if rejected.exists() or rejected.is_symlink():
                        raise ValueError("previous rejected tool bundle already exists")
                    options.bundle.rename(rejected)
        else:
            if len(options.cargo_log) != 2:
                raise ValueError("two completed Cargo JSON logs are required")
            evidence = collect_evidence(workspace, candidate, options.cargo_log, os.environ)
            (state / "coverage.json").write_text(json.dumps(evidence, indent=2) + "\n")
            outputs["admitted"] = str(evidence["admitted"]).lower()
            report.update(status="admitted" if evidence["admitted"] else "not-admitted",
                          refusal_count=len(evidence["refusals"]))
            if evidence["admitted"]:
                # Restore may have left a rejected bundle. Replace only this task-owned delivery directory.
                if options.bundle.exists():
                    raise ValueError("refusing to overwrite a restored bundle; use a separate publication directory")
                options.bundle.mkdir(parents=True)
                for name in TOOLS:
                    shutil.copy2(workspace / "target/release" / name, options.bundle / name)
                manifest = {"inputs": candidate, "evidence": evidence,
                            "tools": output_manifest(options.bundle, candidate["identity"])}
                (options.bundle / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    report["elapsed_seconds"] = time.monotonic() - start
    (state / (options.operation + ".json")).write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report))
    if os.environ.get("GITHUB_OUTPUT"):
        with open(os.environ["GITHUB_OUTPUT"], "a") as output:
            for key, value in outputs.items():
                output.write(f"{key}={value}\n")


def record_cargo_messages(destination, stream, diagnostics):
    """Retain Cargo facts privately while printing compiler diagnostics, never extension env JSON."""
    destination.parent.mkdir(parents=True, exist_ok=True)
    with destination.open("w") as output:
        for line in stream:
            output.write(line)
            message = json.loads(line)
            if message.get("reason") == "compiler-message":
                rendered = message.get("message", {}).get("rendered")
                if rendered:
                    diagnostics.write(rendered)


if __name__ == "__main__":
    import os
    import sys
    try:
        if len(sys.argv) == 3 and sys.argv[1] == "record":
            record_cargo_messages(Path(sys.argv[2]), sys.stdin, sys.stdout)
        else:
            main()
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        # This cache is an accelerator: missing coverage keeps the existing cold compiler build usable.
        print(json.dumps({"status": "unavailable", "detail": str(error)}), file=sys.stderr)
        if os.environ.get("GITHUB_OUTPUT"):
            with open(os.environ["GITHUB_OUTPUT"], "a") as output:
                output.write("eligible=false\nhit=false\nadmitted=false\n")
        if "--state" in sys.argv:
            state = Path(sys.argv[sys.argv.index("--state") + 1])
            state.mkdir(parents=True, exist_ok=True)
            (state / (sys.argv[1] + "-unavailable.json")).write_text(
                json.dumps({"status": "unavailable", "detail": str(error)}, indent=2) + "\n")


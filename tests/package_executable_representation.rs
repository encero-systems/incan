//! Real producer-to-source-unavailable-consumer acceptance for public package execution.

use std::error::Error;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use incan::library_manifest::LibraryManifest;
use incan::library_manifest::published_layout::executable_surface_path;
use incan_semantics_core::executable_representation::SurfaceReader;

mod support;

/// Run the repository-built compiler with the test harness's coherent SDK/provider selection.
fn command(project: &Path) -> Command {
    let mut command = Command::new(support::incan_binary());
    command
        .current_dir(project)
        .env("INCAN_NO_BANNER", "1")
        .env_remove("INCAN_INTERNAL_PROJECT_ROOT")
        .env_remove("INCAN_INTERNAL_MANIFEST_OVERRIDE");
    command
}

/// Require a command to succeed while retaining complete diagnostics for a failed producer or consumer boundary.
fn success(output: Output, phase: &str) -> Result<Output, Box<dyn Error>> {
    if !output.status.success() {
        return Err(format!(
            "{phase} failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(output)
}

/// Publish a library or native consumer through the normal explicit Oven bake boundary.
fn bake(project: &Path) -> Result<(), Box<dyn Error>> {
    let mut build = command(project);
    build.args(["oven", "bake", "--project", "."]);
    support::configure_explicit_oven_bake_command(&mut build)?;
    success(build.output()?, "Oven bake")?;
    Ok(())
}

/// Create one minimal project without sharing mutable source or generated output with another test.
fn project(root: &Path, name: &str, source_name: &str, source: &str, dependencies: &str) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(root.join("src"))?;
    fs::write(
        root.join("loaf.toml"),
        format!("[project]\nname = \"{name}\"\nversion = \"1.2.3\"\n{dependencies}"),
    )?;
    fs::write(root.join("src").join(source_name), source)?;
    Ok(())
}

/// Capture complete artifact bytes and inventory without following symbolic links into external stores.
fn artifact_snapshot(root: &Path) -> Result<std::collections::BTreeMap<std::path::PathBuf, Vec<u8>>, Box<dyn Error>> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = std::collections::BTreeMap::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                pending.push(path);
            } else {
                let bytes = if entry.file_type()?.is_symlink() {
                    fs::read_link(&path)?.to_string_lossy().as_bytes().to_vec()
                } else {
                    fs::read(&path)?
                };
                files.insert(path.strip_prefix(root)?.to_path_buf(), bytes);
            }
        }
    }
    Ok(files)
}

/// Native and non-linking consumers select the same alias target after the producer source has been removed.
#[test]
fn source_unavailable_package_executes_alias_defaults_and_public_closure() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let producer = temporary.path().join("producer");
    let consumer = temporary.path().join("consumer");
    project(
        &producer,
        "arithmetic",
        "lib.incn",
        r#"pub model Pair:
    pub value: int

pub enum Mode:
    Ready
    Idle

pub def base() -> int:
    return 20

pub def doubled(value: int = base()) -> int:
    return value * 2 + 2

def private_secret() -> int:
    return 999
"#,
        "",
    )?;
    bake(&producer)?;
    success(
        command(&producer)
            .args(["build", "--lib", "--locked", "src/lib.incn"])
            .output()?,
        "library publication",
    )?;
    let manifest_path = producer.join("target/lib/arithmetic.incnlib");
    let manifest = LibraryManifest::read_from_path(&manifest_path)?;
    let surface = executable_surface_path(&manifest_path, &manifest).ok_or("manifest has no representation")?;
    let bytes = fs::read(&surface)?;
    assert!(
        !bytes
            .windows(b"private_secret".len())
            .any(|part| part == b"private_secret")
    );
    let doubled = manifest
        .contract_metadata
        .identity_graph
        .canonical_for_public_name("doubled")
        .ok_or("doubled has no manifest identity")?;
    assert!(SurfaceReader::open(&bytes)?.covers(&doubled));
    let source = "from pub::renamed import doubled as answer, Pair as Item, Mode as State\n\ndef main() -> None:\n    item = Item(value=0)\n    if State.Ready == State.Ready:\n        println(answer() + item.value)\n";
    project(
        &consumer,
        "consumer",
        "main.incn",
        source,
        "\n[dependencies]\nrenamed = { path = \"../producer\" }\n",
    )?;
    bake(&consumer)?;
    let native = success(
        command(&consumer).args(["run", "--locked", "src/main.incn"]).output()?,
        "native consumer",
    )?;
    assert_eq!(String::from_utf8_lossy(&native.stdout).trim(), "42");

    // A retained public native derive deliberately fails at rustc after preparation has generated a different
    // public function. An unused private model would be removed by normal native emission and could not fail.
    // Both the prior linked artifact and its executable publication must survive that ordinary rebuild failure.
    let artifact_before = artifact_snapshot(&producer.join("target/lib"))?;
    let authored = fs::read_to_string(producer.join("src/lib.incn"))?;
    fs::write(
        producer.join("src/lib.incn"),
        format!(
            "{}\n@rust.derive(Eq, Hash)\npub model InvalidNativeDerive:\n    pub value: float\n",
            authored.replace("return 20", "return 99")
        ),
    )?;
    let mut rebuild = command(&producer);
    rebuild.args(["oven", "bake", "--project", "."]);
    support::configure_explicit_oven_bake_command(&mut rebuild)?;
    let failure = rebuild.output()?;
    assert!(!failure.status.success(), "invalid native derive unexpectedly compiled");
    let diagnostic = String::from_utf8_lossy(&failure.stderr);
    assert!(
        diagnostic.contains("f64") && (diagnostic.contains("Eq") || diagnostic.contains("Hash")),
        "expected native derive failure, got {diagnostic}"
    );
    assert_eq!(artifact_snapshot(&producer.join("target/lib"))?, artifact_before);
    fs::remove_dir_all(producer.join("src"))?;
    fs::remove_file(producer.join("loaf.toml"))?;
    let retained_native = success(
        command(&consumer).args(["run", "--locked", "src/main.incn"]).output()?,
        "native consumer after failed rebuild and source removal",
    )?;
    assert_eq!(retained_native.stdout, native.stdout);
    let report_path = consumer.join("execution.json");
    let replacement = success(
        command(&consumer)
            .args([
                "build",
                "src/main.incn",
                "--backend",
                "replacement",
                "--report",
                "json",
                "--report-output",
            ])
            .arg(&report_path)
            .output()?,
        "source-unavailable replacement",
    )?;
    assert_eq!(replacement.stdout, native.stdout);
    let local = temporary.path().join("local");
    let local_main = source
        .lines()
        .skip(1)
        .collect::<Vec<_>>()
        .join("\n")
        .replace("Item(value=", "Pair(value=")
        .replace("State.Ready", "Mode.Ready")
        .replace("answer()", "doubled()");
    project(&local, "local", "main.incn", &format!("{authored}\n{local_main}\n"), "")?;
    let local_execution = success(
        command(&local)
            .args(["build", "src/main.incn", "--backend", "replacement"])
            .output()?,
        "equivalent local source",
    )?;
    assert_eq!(local_execution.stdout, replacement.stdout);
    let report: serde_json::Value = serde_json::from_slice(&fs::read(report_path)?)?;
    assert_eq!(report["replacement_execution"]["package_declarations_decoded"], 4);
    assert_eq!(
        report["replacement_execution"]["package_content_bytes_verified"],
        bytes.len()
    );
    assert!(
        report["replacement_execution"]["package_payload_bytes_read"]
            .as_u64()
            .is_some_and(|count| count > 0)
    );
    assert!(consumer.join(".incan/backend/receipt.json").is_file());
    assert_eq!(
        fs::read(surface)?,
        bytes,
        "consumer must never regenerate a dependency representation"
    );
    Ok(())
}

/// A required unavailable call behind a branch refuses before the earlier print or any successful receipt.
#[test]
fn missing_package_representation_refuses_before_output_and_receipt() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let producer = temporary.path().join("producer");
    let consumer = temporary.path().join("consumer");
    project(
        &producer,
        "conditional_provider",
        "lib.incn",
        "pub def answer() -> int:\n    return 42\n",
        "",
    )?;
    bake(&producer)?;
    let manifest_path = producer.join("target/lib/conditional_provider.incnlib");
    let mut manifest = LibraryManifest::read_from_path(&manifest_path)?;
    // Absence is an explicitly valid linking-only package, not corrupt semantic payload bytes.
    manifest.contract_metadata.executable_representation = None;
    manifest.write_to_path(&manifest_path)?;
    fs::remove_dir_all(producer.join("src"))?;
    fs::remove_file(producer.join("loaf.toml"))?;
    project(
        &consumer,
        "consumer",
        "main.incn",
        "from pub::renamed import answer\n\ndef main() -> None:\n    println(\"must not print\")\n    if False:\n        println(answer())\n",
        "\n[dependencies]\nrenamed = { path = \"../producer\" }\n",
    )?;
    let output = command(&consumer)
        .args(["build", "src/main.incn", "--backend", "replacement"])
        .output()?;
    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "program output must remain empty: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("conditional_provider")
            && error.contains("1.2.3")
            && error.contains("executable representation"),
        "{error}"
    );
    assert!(
        !error.contains("INCAN-R988-UNSUPPORTED"),
        "package refusal must not blame a language construct: {error}"
    );
    assert!(!consumer.join(".incan/backend/receipt.json").exists());
    Ok(())
}

/// A real debug bake followed by a release-only tool failure must restore both native and semantic generations.
#[cfg(unix)]
#[test]
fn release_failure_after_new_debug_output_restores_both_consumer_routes() -> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir()?;
    let producer = temporary.path().join("producer");
    let consumer = temporary.path().join("consumer");
    project(
        &producer,
        "arithmetic",
        "lib.incn",
        "pub def answer() -> int:\n    return 41\n",
        "",
    )?;
    let mut initial = command(&producer);
    initial
        .args(["oven", "bake", "--project", "."])
        .env("INCAN_OVEN_BAKE_PROFILES", "all");
    support::configure_explicit_oven_bake_command(&mut initial)?;
    success(initial.output()?, "initial debug and release library publication")?;
    project(
        &consumer,
        "consumer",
        "main.incn",
        "from pub::renamed import answer\n\ndef main() -> None:\n    println(answer())\n",
        "\n[dependencies]\nrenamed = { path = \"../producer\" }\n",
    )?;
    bake(&consumer)?;
    let native_before = success(
        command(&consumer).args(["run", "--locked", "src/main.incn"]).output()?,
        "native consumer before partial rebuild",
    )?;
    assert_eq!(String::from_utf8_lossy(&native_before.stdout).trim(), "41");
    let artifact_before = artifact_snapshot(&producer.join("target/lib"))?;
    let receipt_root = incan::oven::default_receipt_path(&producer)
        .parent()
        .ok_or("receipt directory absent")?
        .to_path_buf();
    let receipts_before = artifact_snapshot(&receipt_root)?;
    let debug = producer.join("target/lib/oven/debug/libarithmetic.rlib");
    let release = producer.join("target/lib/oven/release/libarithmetic.rlib");
    let old_debug = fs::read(&debug)?;
    assert!(release.is_file());
    fs::write(
        producer.join("src/lib.incn"),
        "pub def answer() -> int:\n    return 42\n",
    )?;
    let shim = temporary.path().join("release-failing-rustc");
    let proof = temporary.path().join("new-debug.rlib");
    fs::write(
        &shim,
        r#"#!/bin/sh
is_debug=0
for arg in "$@"; do
    if [ "$arg" = "$RFC123_RELEASE_OUTPUT" ]; then
        echo "RFC123 injected release failure after successful debug output" >&2
        exit 87
    fi
    if [ "$arg" = "$RFC123_DEBUG_OUTPUT" ]; then is_debug=1; fi
done
"$RFC123_REAL_RUSTC" "$@"
result=$?
if [ "$result" -eq 0 ] && [ "$is_debug" -eq 1 ]; then
    cp "$RFC123_DEBUG_OUTPUT" "$RFC123_DEBUG_PROOF" || exit 88
fi
exit "$result"
"#,
    )?;
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755))?;
    let mut rebuild = command(&producer);
    rebuild
        .args(["oven", "bake", "--project", "."])
        .env("INCAN_OVEN_BAKE_PROFILES", "all")
        .env("RUSTC", &shim)
        .env("RFC123_REAL_RUSTC", incan::oven::rustc::resolve_active_rustc()?)
        .env("RFC123_DEBUG_OUTPUT", &debug)
        .env("RFC123_RELEASE_OUTPUT", &release)
        .env("RFC123_DEBUG_PROOF", &proof);
    support::configure_explicit_oven_bake_command(&mut rebuild)?;
    let failed = rebuild.output()?;
    assert!(!failed.status.success());
    assert!(
        String::from_utf8_lossy(&failed.stderr).contains("RFC123 injected release failure"),
        "{}",
        String::from_utf8_lossy(&failed.stderr)
    );
    assert_ne!(
        fs::read(proof)?,
        old_debug,
        "the debug native artifact must actually have changed before release failed"
    );
    assert_eq!(artifact_snapshot(&producer.join("target/lib"))?, artifact_before);
    // Immutable intermediate cache entries may remain, but all pre-existing successful receipt bytes must survive.
    let receipts_after = artifact_snapshot(&receipt_root)?;
    for (path, bytes) in receipts_before {
        assert_eq!(
            receipts_after.get(&path),
            Some(&bytes),
            "receipt changed: {}",
            path.display()
        );
    }
    fs::remove_dir_all(producer.join("src"))?;
    fs::remove_file(producer.join("loaf.toml"))?;
    let native = success(
        command(&consumer).args(["run", "--locked", "src/main.incn"]).output()?,
        "native consumer after partial rebuild failure",
    )?;
    let replacement = success(
        command(&consumer)
            .args(["build", "src/main.incn", "--backend", "replacement"])
            .output()?,
        "replacement consumer after partial rebuild failure",
    )?;
    assert_eq!(native.stdout, native_before.stdout);
    assert_eq!(replacement.stdout, native_before.stdout);
    Ok(())
}

/// A consumer names only pricing; checked catalog identity and native routes survive a public type facade.
#[test]
fn source_unavailable_type_facade_signatures_run_natively_and_without_linking() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let catalog = temporary.path().join("catalog");
    let facade = temporary.path().join("facade");
    let pricing = temporary.path().join("pricing");
    let consumer = temporary.path().join("consumer");
    project(
        &catalog,
        "catalog",
        "lib.incn",
        "pub model Product:\n    pub value: int\n\npub def first_product() -> Product:\n    return Product(value=42)\n",
        "",
    )?;
    bake(&catalog)?;
    project(
        &facade,
        "facade",
        "lib.incn",
        "pub from pub::catalog import Product, first_product\n",
        "\n[dependencies]\ncatalog = { path = \"../catalog\" }\n",
    )?;
    bake(&facade)?;
    project(
        &pricing,
        "pricing",
        "lib.incn",
        "from pub::types import Product, first_product\n\npub def first() -> Product:\n    return first_product()\n\npub def keep(values: list[Product]) -> list[Product]:\n    return values\n\npub def quote(product: Product) -> int:\n    return product.value\n",
        "\n[dependencies]\ntypes = { path = \"../facade\" }\n",
    )?;
    bake(&pricing)?;
    project(
        &consumer,
        "consumer",
        "main.incn",
        "from pub::pricing import first, quote\n\ndef main() -> None:\n    println(quote(first()))\n",
        "\n[dependencies]\npricing = { path = \"../pricing\" }\n",
    )?;
    bake(&consumer)?;
    let native = success(
        command(&consumer).args(["run", "--locked", "src/main.incn"]).output()?,
        "pricing-only native consumer",
    )?;
    assert_eq!(String::from_utf8_lossy(&native.stdout).trim(), "42");
    let artifacts = [&catalog, &facade, &pricing]
        .into_iter()
        .map(|producer| artifact_snapshot(&producer.join("target/lib")))
        .collect::<Result<Vec<_>, _>>()?;
    for producer in [&catalog, &facade, &pricing] {
        fs::remove_dir_all(producer.join("src"))?;
        fs::remove_file(producer.join("loaf.toml"))?;
    }
    let retained_native = success(
        command(&consumer).args(["run", "--locked", "src/main.incn"]).output()?,
        "source-unavailable pricing-only native consumer",
    )?;
    assert_eq!(retained_native.stdout, native.stdout);
    // This project has never been compiled. Its first publication must link only the admitted package artifacts;
    // re-running the existing executable above would not prove the source-unavailable linking boundary. This native
    // probe also exercises nested foreign signatures; nominal list construction remains outside the direct profile.
    let fresh_consumer = temporary.path().join("fresh_consumer");
    project(
        &fresh_consumer,
        "fresh_consumer",
        "main.incn",
        "from pub::pricing import first, keep, quote\n\ndef main() -> None:\n    product = first()\n    values = keep([product])\n    println(quote(values[0]))\n",
        "\n[dependencies]\npricing = { path = \"../pricing\" }\n",
    )?;
    assert!(!fresh_consumer.join("target").exists());
    bake(&fresh_consumer)?;
    let freshly_linked = success(
        command(&fresh_consumer)
            .args(["run", "--locked", "src/main.incn"])
            .output()?,
        "fresh pricing-only native consumer after all producer sources were removed",
    )?;
    assert_eq!(freshly_linked.stdout, native.stdout);
    let replacement = success(
        command(&consumer)
            .args(["build", "src/main.incn", "--backend", "replacement"])
            .output()?,
        "source-unavailable pricing-only replacement consumer",
    )?;
    assert_eq!(replacement.stdout, native.stdout);
    for (producer, before) in [&catalog, &facade, &pricing].into_iter().zip(artifacts) {
        assert_eq!(artifact_snapshot(&producer.join("target/lib"))?, before);
    }
    Ok(())
}

/// A producer's union wrapper and payload order survive foreign nominal routes and a source-free facade.
#[test]
fn source_unavailable_native_union_preserves_producer_representation_through_facade() -> Result<(), Box<dyn Error>> {
    use incan::library_manifest::{NativeUnionOwnerExport, TypeRef};

    let temporary = tempfile::tempdir()?;
    let catalog = temporary.path().join("catalog");
    let pricing = temporary.path().join("pricing");
    let facade = temporary.path().join("facade");
    project(
        &catalog,
        "union_catalog",
        "lib.incn",
        "pub model Product:\n    pub value: int\n",
        "",
    )?;
    bake(&catalog)?;
    project(
        &pricing,
        "union_pricing",
        "lib.incn",
        "pub from answer import Answer, Surcharge, choose, score\npub from pub::catalog import Product\n",
        "\n[dependencies]\ncatalog = { path = \"../catalog\" }\n",
    )?;
    fs::write(
        pricing.join("src/answer.incn"),
        r#"from pub::catalog import Product

pub model Surcharge:
    pub value: int

pub type Answer = Union[Product, Surcharge, int]

pub def choose(use_product: bool) -> Answer:
    if use_product:
        return Product(value=35)
    return 7

pub def score(value: Answer) -> int:
    match value:
        Product(item) => return item.value
        Surcharge(item) => return item.value
        int(number) => return number
"#,
    )?;
    bake(&pricing)?;
    project(
        &facade,
        "union_facade",
        "lib.incn",
        "pub from pub::pricing import Answer as Value, Product, Surcharge as Fee, choose as pick, score as measure\n",
        "\n[dependencies]\npricing = { path = \"../pricing\" }\n",
    )?;
    bake(&facade)?;

    let pricing_manifest = LibraryManifest::read_from_path(&pricing.join("target/lib/union_pricing.incnlib"))?;
    let answer = pricing_manifest
        .exports
        .type_aliases
        .iter()
        .find(|alias| alias.name == "Answer")
        .map(|alias| &alias.target)
        .or_else(|| {
            pricing_manifest
                .exports
                .aliases
                .iter()
                .find(|alias| alias.name == "Answer")
                .and_then(|alias| alias.projected_type.as_ref())
        })
        .ok_or("pricing Answer type projection absent")?;
    let TypeRef::NativeUnion(producer_union) = answer else {
        return Err("producer alias did not retain its emitted union descriptor".into());
    };
    assert_eq!(producer_union.owner, NativeUnionOwnerExport::ContainingArtifact);
    assert_eq!(producer_union.members.len(), 3);
    assert_eq!(producer_union.local_nominals.len(), 1);
    assert!(
        pricing_manifest
            .contract_metadata
            .native_unions
            .contains(producer_union)
    );
    let generated_root = fs::read_to_string(pricing.join("target/lib/src/lib.rs"))?;
    assert!(generated_root.contains(&format!("enum {}", producer_union.rust_name)));

    let facade_manifest = LibraryManifest::read_from_path(&facade.join("target/lib/union_facade.incnlib"))?;
    let forwarded_type = facade_manifest
        .exports
        .type_aliases
        .iter()
        .find(|alias| alias.name == "Value")
        .map(|alias| &alias.target)
        .or_else(|| {
            facade_manifest
                .exports
                .aliases
                .iter()
                .find(|alias| alias.name == "Value")
                .and_then(|alias| alias.projected_type.as_ref())
        })
        .ok_or("facade Value type projection absent")?;
    let TypeRef::NativeUnion(forwarded_union) = forwarded_type else {
        return Err("facade discarded the producer's native union descriptor".into());
    };
    assert_eq!(forwarded_union.rust_name, producer_union.rust_name);
    assert_eq!(forwarded_union.local_nominals, producer_union.local_nominals);
    assert_eq!(forwarded_union.members.len(), producer_union.members.len());
    let NativeUnionOwnerExport::SelectedArtifact(owner) = &forwarded_union.owner else {
        return Err("facade reassigned the union to its own artifact".into());
    };
    for (original, forwarded) in producer_union.members.iter().zip(&forwarded_union.members) {
        match original {
            TypeRef::Named { name, origin: None } if producer_union.local_nominals.contains_key(name) => {
                let TypeRef::Named {
                    origin: Some(origin), ..
                } = forwarded
                else {
                    return Err("facade lost the producer-local nominal's selected origin".into());
                };
                assert_eq!(&origin.provider, owner);
                assert_eq!(producer_union.local_nominals.get(name), Some(&origin.canonical));
            }
            TypeRef::Named {
                origin: Some(origin), ..
            } => {
                let TypeRef::Named {
                    origin: Some(forwarded_origin),
                    ..
                } = forwarded
                else {
                    return Err("facade lost the foreign nominal's selected origin".into());
                };
                assert_eq!(forwarded_origin, origin);
            }
            _ => assert_eq!(forwarded, original),
        }
    }
    let pricing_edge = facade_manifest
        .contract_metadata
        .provider
        .provider_dependencies
        .iter()
        .find(|dependency| dependency.dependency_key == "pricing")
        .ok_or("facade's selected pricing artifact absent")?;
    assert_eq!(owner.name, pricing_edge.provider_name);
    assert_eq!(owner.version, pricing_edge.provider_version);
    assert_eq!(owner.digest, pricing_edge.artifact_digest);
    assert_eq!(
        owner.feature_projection,
        pricing_manifest.contract_metadata.provider.active_features
    );
    assert!(facade_manifest.contract_metadata.native_unions.is_empty());

    let artifacts = [&catalog, &pricing, &facade]
        .into_iter()
        .map(|root| Ok((root, artifact_snapshot(&root.join("target/lib"))?)))
        .collect::<Result<Vec<_>, Box<dyn Error>>>()?;
    for root in [&catalog, &pricing, &facade] {
        fs::remove_dir_all(root.join("src"))?;
        fs::remove_file(root.join("loaf.toml"))?;
    }

    // This consumer has never existed before source removal: its native output cannot be a retained executable.
    let consumer = temporary.path().join("fresh_consumer");
    project(
        &consumer,
        "union_consumer",
        "main.incn",
        r#"from pub::bridge import Value as Reading, Product as Item, Fee as Charge, pick, measure

def read(value: Reading) -> int:
    match value:
        Item(item) => return item.value
        Charge(item) => return item.value
        int(number) => return number

def main() -> None:
    println(measure(pick(true)) + measure(pick(false)))
    println(read(pick(true)) + read(pick(false)))
    println(measure(Item(value=20)) + measure(22))
    println(measure(Charge(value=20)) + read(Charge(value=22)))
"#,
        "\n[dependencies]\nbridge = { path = \"../facade\" }\n",
    )?;
    bake(&consumer)?;
    let run = success(
        command(&consumer).args(["run", "src/main.incn", "--locked"]).output()?,
        "fresh native union consumer after all producer source removal",
    )?;
    assert_eq!(run.stdout, b"42\n42\n42\n42\n");
    for (root, before) in artifacts {
        assert_eq!(artifact_snapshot(&root.join("target/lib"))?, before);
    }
    Ok(())
}

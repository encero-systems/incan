# Boundary Parity Fixtures

These fixtures keep boundary-identity regressions compact. They model failure families from the 0.3 RC cycle onward with small synthetic packages instead of adding one downstream-shaped regression for every historical bug.

- `boundary_parity_preserves_dependency_owned_union_helpers_through_facade` covers provider-owned union wrappers through facades, aliases, list arguments, methods, and generated Rust ownership.
- `boundary_parity_preserves_decorated_alias_partial_identity_through_facade` covers decorated callable identity, aliases, partial presets, direct source imports, facade source imports, source test batches, and provider/facade/consumer package boundaries.
- `boundary_parity_preserves_enum_method_defaults_through_facade` covers dependency-owned enum methods and materialized default arguments through provider/facade/consumer package boundaries.
- `boundary_parity_preserves_absolute_crate_public_types_issue882` covers absolute sibling-module imports, public model fields, enum variants, direct check/build, library build/re-export, and test-batch parity.
- `boundary_parity_activates_dependency_vocab_across_check_fmt_and_test` covers dependency-provided vocab activation through `--check`, `fmt --check`, and `incan test`.
- `test_qualified_partial_constructor_presets_cross_package_const_metadata_issue699` covers source-qualified constructor partials whose generated const-safe metadata depends on provider-owned model fields.
- Existing synthetic Rust callback tests in `cli_rust_interop_tests` cover Rust metadata/callback planning without adding heavyweight downstream crates to Incan's regression lane.

When adding boundary coverage, extend these fixture families before adding another one-off downstream-shaped test. The goal is fewer tests with stronger semantic coverage, not a larger slow suite.

## Public-boundary evidence and remaining fixture work

Package-boundary behavior across a materialized dependency whose source and authored manifest have been removed is proved by `loaves/toolchain/incan-cli/tests/package_executable_representation.rs`; package consumers whose providers are baked in the fixture are behavior fixtures in the `*_dependencies` areas under `../behavior/`. Signed archive delivery is outside both. Broader public-boundary work remains tracked through [#989](https://github.com/encero-systems/incan/issues/989) and its package, metadata and Rust-interop packets.

The existing regressions below are legacy-path evidence. Several build generated Rust, materialize a legacy `.incnlib`, or invoke a Cargo-compatible consumer artifact. They remain valuable characterization tests, but none of them proves the direct route merely because it passes.

### Remaining packet 1 coverage — [#1260 package and local-module import execution](https://github.com/encero-systems/incan/issues/1260)

| Existing evidence | What it characterizes today | Later #989 fixture needed |
| --- | --- | --- |
| `boundary_parity_preserves_absolute_crate_public_types_issue882` in `loaves/toolchain/incan-cli/tests/rfc031_boundary_parity_tests.rs` | A local `crate` import survives direct build, library re-export, and source test-batch compilation. | One direct-route case that executes both a local-module import and a package-consumer import, then compares their source-observable result with the reference route. |
| `build_lib_artifacts_and_consumer_alias_typecheck` and the `check_reports_*pub_*` cases in `loaves/toolchain/incan-cli/tests/rfc031_manifest_diagnostics_tests.rs` | Legacy `.incnlib` manifest lookup, `pub::` import acceptance, and current missing-library/export/artifact diagnostics. | A package-consumer case with checked public metadata and a public diagnostic/source-map record for an unavailable package, export, or version/profile incompatibility. |
| `generated_library_and_pub_dependency_consumer_artifacts_match_baseline` in `loaves/compiler/incan_driver/tests/generated_rust_artifact_tests.rs` and `examples/advanced/library_package/` | The current producer/consumer artifact shape and a normal package-consumer flow. | A receipt-bound package fixture that proves the consumer used the selected caller/package contract, not generated project layout or executor-local import lookup. |

### Remaining packet 2 coverage — [#1261 facade/re-export and checked public metadata](https://github.com/encero-systems/incan/issues/1261)

| Existing evidence | What it characterizes today | Later #989 fixture needed |
| --- | --- | --- |
| `boundary_parity_preserves_decorated_alias_partial_identity_through_facade` and `boundary_parity_preserves_enum_method_defaults_through_facade` in `loaves/toolchain/incan-cli/tests/rfc031_boundary_parity_tests.rs` | Facade imports, aliases, partials, defaults, and source test batches across producer/facade/consumer packages. | A direct-route facade chain whose direct import, alias, and re-export resolve to the same compiler-owned public identity and preserve the selected checked public contract. |
| `build_pub_consumer_imports_public_alias_of_imported_item_issue617`, `build_lib_materializes_facade_decorator_metadata_projection_issue695`, in `loaves/toolchain/incan-cli/tests/cli_provider_boundary_tests.rs` and `loaves/toolchain/incan-cli/tests/cli_decorator_and_partial_tests.rs`, and `tools_metadata_api_reports_public_import_aliases` in `loaves/toolchain/incan-cli/tests/cli_surface_tests.rs` | Current public-alias projection and checked API metadata derived from source. | A metadata assertion over the versioned package/caller-facet contract, including export identity, supported type projection, and explicit unsupported states rather than generated-Rust names or private compiler structures. |
| `workspaces/docs-site/docs/tooling/reference/checked_api_metadata.md` | The current source-checked metadata command and its explicit separation from `.incnlib` artifact inspection. | A contract check that distinguishes source/project metadata from the later caller/package metadata without making either generated Rust or private HIR/Body IR a public interface. |

### Remaining packet 3 coverage — [#1262 Rust-interop call and diagnostic parity](https://github.com/encero-systems/incan/issues/1262)

| Existing evidence | What it characterizes today | Later #989 fixture needed |
| --- | --- | --- |
| `loaves/compiler/incan_emit/tests/codegen_snapshots/rfc041_rust_coercions.incn`, `rfc041_interop_into_via.incn`, `rust_interop_associated_functions.incn`, `rust_interop_field_access.incn`, and `rfc043_imported_trait_associated_type.incn` | Current Rust-import, coercion, associated-call, field-access, and trait-associated-type emission behavior. | Call and coercion behavior fixtures that run on the direct route; generated Rust snapshots remain inspection evidence only. |
| `compiled_provider_preserves_shared_rust_interop_contracts_issues834_835_961` in `loaves/toolchain/incan-cli/tests/rfc031_rust_interop_tests.rs`, the Rust metadata coverage in `loaves/toolchain/incan-cli/tests/cli_rust_borrow_interop_tests.rs` and the Rust callback coverage in `loaves/toolchain/incan-cli/tests/cli_rust_generic_interop_tests.rs` | Current provider and compiler interop planning across a legacy compiled path. | A caller-facet case that proves supported Rust-host call shapes and refuses non-representable exports or boundary types before publishing an invalid consumer artifact. |
| `loaves/compiler/incan_driver/tests/generated_rust_native_consumer_tests.rs` and `workspaces/docs-site/docs/language/how-to/rust_interop.md` | The existing generated-Rust native-consumer surface and the authored Rust-interop guidance. | Public diagnostic/source-map assertions that distinguish Incan domain, conversion, host-capability, version, and backend/runtime failures without deriving source identity from emitted names. |

### Required evidence before a boundary case counts as proved on the direct route

Every future case is a behavior fixture: an Incan program whose header declares what a run must show. Its run must bind together:

- the #986 backend selection and execution receipt of the route that ran, with matching selection and execution identities;
- the selected versioned caller/package contract: package and caller-facet identity, ABI and manifest schema versions, compiler/runtime compatibility, target/profile compatibility, type projection, runtime/helper and linkage requirements, and explicit unknown or unsupported states;
- public package/caller receipt and diagnostic/source-map records that bind the selected versioned contract to those selection/execution receipts, without making Oven bake, Loaf adoption, or Cargo replacement a prerequisite;
- the program's observable result, compared with what the fixture declares. A generated-Rust snapshot, generated Cargo layout, or a receipt alone cannot substitute for it.

The contract evidence must remain compiler-owned public output. It must not expose private HIR, Body IR, compiler-session state, backend implementation details, generated Rust identifiers, or generated project layout as compatibility inputs.

### Remaining evidence gaps

- `loaves/toolchain/incan-cli/tests/package_executable_representation.rs` runs source-unavailable packages natively on the legacy route, including canonical aliases, defaults, public nominal context, type facades and unions. It has no non-linking execution and no case for a dependency that lacks an executable representation. It does not imply complete package, metadata or Rust-interop coverage for every row in the packets above, and none of its cases is receipt-aware evidence for the direct route.
- Signed archive publication and transport remain outside the current materialized-package execution contract, as recorded by [RFC 123](../../../../../workspaces/docs-site/docs/RFCs/123_package_executable_representation.md) and [#1339](https://github.com/encero-systems/incan/issues/1339).

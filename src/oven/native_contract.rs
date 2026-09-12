//! The Cargo-free wire contract the direct-rustc route publishes and consumes.
//!
//! This module names the payload, suite, shard, foundation and toolchain-data types the native route reads and
//! writes, separately from the baker that currently produces them. Every one of them is still **declared** by
//! `legacy_cargo`, and re-exported here rather than redeclared: a publisher and the consumer that reads its output
//! must hold one type, and two same-named structs with identical fields are not one type. Redeclaring them made
//! `OvenProjectExtensionPayload` unusable across that boundary, with the compiler reporting `expected
//! OvenProjectExtensionPayload, found a different OvenProjectExtensionPayload`.
//!
//! The direction of travel is the other way round: when `legacy_cargo` is retired at Gate 8 the declarations move
//! here and the re-export disappears. Keeping this module as the name the native route imports means that move is a
//! change to one file rather than to every consumer.
//!
//! The re-export list is the whole contract, not only the part today's consumers reach, so `unused_imports` is
//! allowed here: trimming it to current callers would make the module a moving target and hide which types the
//! native route owns.

#[allow(unused_imports)]
pub(crate) use super::legacy_cargo::{
    OVEN_COMPILER_TEST_SUITE_FOUNDATION_SCHEMA_VERSION, OVEN_COMPILER_TEST_SUITE_SCHEMA_VERSION,
    OVEN_COMPILER_TEST_SUITE_SHARD_SCHEMA_VERSION, OVEN_COMPILER_TEST_SUITE_SHARD_SCHEMA_VERSION_V1,
    OVEN_COMPILER_TEST_SUITE_TOOLCHAIN_DATA_SCHEMA_VERSION, OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION,
    OvenCompilerTestSuiteArtifactClosure, OvenCompilerTestSuiteFoundationPayload,
    OvenCompilerTestSuiteFoundationReference, OvenCompilerTestSuitePayload, OvenCompilerTestSuiteShardPayload,
    OvenCompilerTestSuiteShardReference, OvenCompilerTestSuiteTarget, OvenCompilerTestSuiteTargetKey,
    OvenCompilerTestSuiteToolchainDataPayload, OvenCompilerTestSuiteToolchainDataReference,
    OvenCompilerTestSuiteToolchainLoafGenerationReference, OvenCompilerWorkspaceLibrary,
    OvenCompilerWorkspaceLibraryKey, OvenProjectExtensionPayload, OvenProjectRegistrySourceDependency,
};

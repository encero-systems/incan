//! Library manifest tests, one module per subject; `helpers` holds the shared fixtures.

use std::collections::{BTreeMap, BTreeSet};

use crate::api_metadata::{
    ApiAlias, ApiDeclaration, ApiFunction, ApiModel, ApiNewtype, ApiTrait, CHECKED_API_METADATA_SCHEMA_VERSION,
    CheckedApiMetadata, CheckedApiMetadataPackage, SourceAnchor, SourceSpan, materialize_api_alias_projections,
    materialize_checked_api_public_namespaces,
};

use super::*;

mod helpers;

mod export_round_trips;
mod formats_and_metadata;
mod identity_graph;
mod identity_validation;
mod vocab_and_scoped_surfaces;

use helpers::{legacy_manifest_fixture, published_declaration_identity, source_identity, source_member_identity};

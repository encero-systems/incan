//! Persistent compiler row schemas, encoded by the same standard-library Crumb codec as the reference.

use crate::general_rows::description_from_fields;
use internal_models_bindings::{Description, create_budget, encode_metadata};
use std::sync::OnceLock;

pub(crate) struct Shape {
    pub selector: &'static [u8],
    path: &'static str,
    pub fields: &'static [(&'static [u8], &'static str, &'static str)],
    logical: OnceLock<Description>,
    encoded: OnceLock<Option<Vec<u8>>>,
}

impl Shape {
    /// Declare a fixed schema in original field order.
    const fn new(
        selector: &'static [u8],
        path: &'static str,
        fields: &'static [(&'static [u8], &'static str, &'static str)],
    ) -> Self {
        Self {
            selector,
            path,
            fields,
            logical: OnceLock::new(),
            encoded: OnceLock::new(),
        }
    }

    /// Share the immutable schema used by both hashing and row admission.
    pub fn description(&self) -> &Description {
        self.logical.get_or_init(|| {
            description_from_fields(
                self.selector,
                self.path,
                self.fields.iter().map(|&(id, name, storage)| (id, name, storage, true)),
            )
        })
    }

    /// Encode the fixed schema once under the standard row writer's independent boundary.
    pub fn metadata(&self) -> Option<&[u8]> {
        self.encoded
            .get_or_init(|| {
                let mut budget = create_budget(i64::MAX, i64::MAX, i64::MAX).ok()?;
                encode_metadata(&self.description().metadata, &mut budget).ok()
            })
            .as_deref()
    }
}

pub(crate) static CHECKER: Shape = Shape::new(
    b"checkr\0",
    "incan.checker-fields/10",
    &[
        (b"family", "family", "string"),
        (b"fields", "fields", "list<string>"),
        (b"owner_", "owner", "string"),
        (b"profil", "profile", "string"),
        (b"root__", "root", "string"),
    ],
);
pub(crate) static GRAPH_BODY: Shape = Shape::new(
    b"grbody\0",
    "incan.checker-graph-body/1",
    &[
        (b"bodyir", "body_ir", "binary"),
        (b"final_", "final", "binary"),
        (b"ident_", "identity", "string"),
        (b"local_", "local", "binary"),
    ],
);
pub(crate) static GRAPH_NAMESPACE: Shape = Shape::new(
    b"grnams\0",
    "incan.checker-graph-namespace/1",
    &[
        (b"bodyir", "body_ir", "binary"),
        (b"iface_", "interface", "binary"),
        (b"inputs", "lowering_inputs", "list<string>"),
        (b"module", "module", "string"),
    ],
);
pub(crate) static NAMESPACE: Shape = Shape::new(
    b"cmpnsp\0",
    "incan.checker-compiled-namespace/1",
    &[
        (b"bodyir", "body_ir", "binary"),
        (b"deps__", "dependencies", "list<string>"),
        (b"iface_", "interface", "binary"),
        (b"inputs", "lowering_inputs", "list<string>"),
        (b"module", "module", "string"),
    ],
);
pub(crate) static VALUE: Shape = Shape::new(
    b"cmpval\0",
    "incan.checker-compiled-value/1",
    &[
        (b"bodyir", "body_ir", "binary"),
        (b"declar", "declaration", "binary"),
        (b"deps__", "dependency_reads", "list<string>"),
        (b"final_", "final", "binary"),
        (b"ident_", "identity", "string"),
        (b"local_", "local", "binary"),
        (b"module", "module", "string"),
        (b"name__", "name", "string"),
    ],
);
pub(crate) static FALLBACK: Shape = Shape::new(
    b"cmpfbk\0",
    "incan.checker-compiled-fallback/1",
    &[
        (b"ident_", "identity", "option<string>"),
        (b"module", "module", "string"),
        (b"name__", "name", "string"),
        (b"reason", "reason", "string"),
    ],
);

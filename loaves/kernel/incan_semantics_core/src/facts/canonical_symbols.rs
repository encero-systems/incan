//! Module identities, symbol namespaces and origins, and canonical symbol identities.

use super::*;

/// Render a module path into the identity string used by HIR, Body IR, and declaration identities.
///
/// One spelling, in one place: an empty path is a real case (a module checked without a path), and the frontend and
/// the data model silently disagreeing about whether that is `"<module>"` or `""` would produce two identities for
/// one declaration.
pub fn module_identity_for_path(module_path: &[String]) -> String {
    if module_path.is_empty() {
        "<module>".to_string()
    } else {
        module_path.join("::")
    }
}

/// Which of RFC 120's three namespaces a binding lives in.
///
/// Namespaces are distinguished by *how* a name is looked up, not by what kind of thing it names: a model name and a
/// function name share one namespace, exactly as ordinary Python-like lexical lookup expects. Carrying this in an
/// identity is what keeps a field named `items` and a local named `items` from ever comparing equal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolNamespace {
    /// Module-level declarations, imports, aliases and re-exports, bare enum variant names, generic binders,
    /// parameters and receivers, and locals. Looked up innermost scope outward, then the builtin fallback tier.
    OrdinaryLexical,
    /// Fields, methods, computed properties, method aliases, and qualified enum variants. Reached `.`-directed from a
    /// resolved owner type, never through the scope chain.
    Member,
    /// Project module paths, the `std` root, `rust::` crate roots, and `pub::` library roots. Path-directed from a
    /// namespace root.
    ModulePath,
}

/// What owns a declaration, independent of who references it.
///
/// An import, alias, or re-export carries its *target's* origin, never the referencing module's. That is the property
/// that makes three different spellings of one declaration compare equal.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolOrigin {
    /// A project source module, by canonical module path.
    Module(Vec<String>),
    /// A `pub::` library, and the module path owning the declaration inside it.
    Package {
        /// Library root name.
        library: String,
        /// Module path within the library.
        module_path: Vec<String>,
    },
    /// A `rust::` crate root and item path.
    RustCrate(Vec<String>),
    /// The compiler-owned builtin registry beneath the ordinary lexical scope chain.
    Builtin,
}

/// Distinguishes bindings that are not unique within their origin.
///
/// Module-level declarations are already unique within their origin and carry no discriminant. Locals, parameters,
/// receivers, and generic binders are not: two `x` bindings in sibling blocks of one module must not collapse to one
/// identity, so those carry the scope that introduced them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ScopeDiscriminant(pub usize);

/// RFC 120 canonical symbol identity: what a resolved reference *means*.
///
/// Assigned once at a declaration site and unchanged by how the declaration is later referenced. A local declaration,
/// an import, an alias, and a re-export of one declaration all carry this same value; none of them creates a second
/// identity for the thing they name.
///
/// Two properties are load-bearing. Equality is decidable structurally, without comparing source spellings or emitted
/// names — no phase may recover what a reference means by parsing generated Rust. And identity is stable across the
/// stages of *one* compilation, not across edits: [`Self::declaration_span`] moves when the file does, so a consumer
/// needing cross-edit continuity must re-resolve rather than cache.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CanonicalSymbolId {
    /// Which namespace the binding lives in.
    pub namespace: SymbolNamespace,
    /// The module, package, crate, or registry owning the declaration.
    pub origin: SymbolOrigin,
    /// The spelling at the *declaration* site, never at a reference site.
    pub declaration_name: String,
    /// Declaration category.
    pub kind: SemanticSourceTargetKind,
    /// Present only for bindings that are not unique within their origin.
    pub scope_discriminant: Option<ScopeDiscriminant>,
    /// Provenance anchor: the one declaration site.
    pub declaration_span: crate::HirSourceSpan,
}

/// Checked, non-positional context needed to project a nested canonical identity across edits.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CanonicalStableDeclarationContext {
    /// Nearest named semantic declaration owning the nested target.
    pub owner: CanonicalSymbolId,
    /// Ordinal among same-name, same-kind bindings inside that owner.
    pub binding_ordinal: u32,
}

impl fmt::Display for CanonicalStableDeclarationContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "owner={} ordinal={}",
            self.owner.render_compact(),
            self.binding_ordinal
        )
    }
}

impl CanonicalSymbolId {
    /// Build the identity of a module-level declaration in a project source module.
    ///
    /// Module-level declarations are unique within their origin, so this deliberately takes no scope discriminant.
    pub fn module_declaration(
        module_path: Vec<String>,
        declaration_name: impl Into<String>,
        kind: SemanticSourceTargetKind,
        declaration_span: crate::HirSourceSpan,
    ) -> Self {
        Self {
            namespace: SymbolNamespace::OrdinaryLexical,
            origin: SymbolOrigin::Module(module_path),
            declaration_name: declaration_name.into(),
            kind,
            scope_discriminant: None,
            declaration_span,
        }
    }

    /// Return the owning module path when this identity is owned by a project source module.
    pub fn module_path(&self) -> Option<&[String]> {
        match &self.origin {
            SymbolOrigin::Module(path) => Some(path),
            _ => None,
        }
    }

    /// Render a deterministic, compact single-line spelling for maintainer-facing snapshots.
    ///
    /// This retains expansion identity for snapshots and internal cycle guards. The shape is
    /// `<kind>:<origin>::<name>[#<scope>]@<start>..<end>`, with member- and path-namespace identities prefixed by
    /// their namespace so a member and a lexical binding sharing a spelling render visibly differently. Generated
    /// identities append `/expansion.<hex>`.
    pub fn render_compact(&self) -> String {
        let origin = match &self.origin {
            SymbolOrigin::Module(path) => module_identity_for_path(path),
            SymbolOrigin::Package { library, module_path } => {
                let mut parts = vec![format!("pub::{library}")];
                parts.extend(module_path.iter().cloned());
                parts.join("::")
            }
            SymbolOrigin::RustCrate(path) => format!("rust::{}", path.join("::")),
            SymbolOrigin::Builtin => "builtin".to_string(),
        };
        let namespace = match self.namespace {
            SymbolNamespace::OrdinaryLexical => "",
            SymbolNamespace::Member => "member/",
            SymbolNamespace::ModulePath => "path/",
        };
        let scope = self
            .scope_discriminant
            .map(|ScopeDiscriminant(scope)| format!("#{scope}"))
            .unwrap_or_default();
        format!(
            "{namespace}{}:{origin}::{}{scope}@{}..{}{}",
            self.kind.as_str(),
            self.declaration_name,
            self.declaration_span.start,
            self.declaration_span.end,
            self.declaration_span
                .expansion
                .map(|value| format!("/expansion.{}", hex::encode(value)))
                .unwrap_or_default()
        )
    }
}

/// Stable package-owned module identity; unlike a source path, it cannot collide between packages.
pub fn package_module_identity(library: &str, module_path: &[String]) -> String {
    format!("pub::{library}::{}", crate::module_identity_for_path(module_path))
}

/// Resolve the physical module scope of an existing canonical identity without minting a new semantic identity.
pub fn canonical_module_identity(identity: &CanonicalSymbolId) -> Option<String> {
    match &identity.origin {
        SymbolOrigin::Module(path) => Some(crate::module_identity_for_path(path)),
        SymbolOrigin::Package { library, module_path } => Some(package_module_identity(library, module_path)),
        SymbolOrigin::RustCrate(_) | SymbolOrigin::Builtin => None,
    }
}

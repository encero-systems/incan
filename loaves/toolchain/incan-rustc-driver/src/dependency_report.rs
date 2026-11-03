//! The settled dependency closure of one unit, as `incan inspect dependencies` prints it.
//!
//! The closure is the one compilation links: checking and planning settle it exactly as they do for a build. Each
//! reached package lists the declarations that entered it from another package or from a requested root, the chain
//! of declarations that requested each one, and why that chain's root was requested.

use std::collections::BTreeMap;
use std::path::Path;

use incan_driver::unit::UnitKind;
use incan_semantics_core::{CanonicalSymbolId, SymbolNamespace, SymbolOrigin};

/// Why a package declaration became a root of the unit's dependency request.
#[derive(Debug, Clone)]
pub(crate) enum RootOrigin {
    /// The unit's checked source names it, directly or through a checked re-export, adoption or coercion.
    Source,
    /// A comparison the unit's bodies perform selects this trait default.
    ComparisonDefault,
    /// A primitive operation of an already requested declaration selects this adapter.
    PrimitiveAdapter,
    /// Planning selected this runtime operation at a source location.
    Runtime {
        path: String,
        file: String,
        line: i64,
        column: i64,
    },
}

impl RootOrigin {
    /// The origin in words, with a runtime operation's location relative to the project.
    fn render(&self, project_root: &Path) -> String {
        match self {
            Self::Source => "checked source".to_owned(),
            Self::ComparisonDefault => "a selected comparison default".to_owned(),
            Self::PrimitiveAdapter => "a selected primitive adapter".to_owned(),
            Self::Runtime {
                path,
                file,
                line,
                column,
            } => {
                let file = Path::new(file);
                let file = file.strip_prefix(project_root).unwrap_or(file);
                format!("runtime operation `{path}` at {}:{line}:{column}", file.display())
            }
        }
    }
}

/// Render the report: a summary line, then each reached package with its entering declarations.
pub(crate) fn render(
    package: &str,
    kind: UnitKind,
    reached: &BTreeMap<String, Vec<Vec<CanonicalSymbolId>>>,
    origins: &BTreeMap<CanonicalSymbolId, RootOrigin>,
    project_root: &Path,
) -> String {
    let plural = if reached.len() == 1 { "" } else { "s" };
    let mut text = format!(
        "{} `{package}` reaches {} package{plural}\n",
        kind.as_str(),
        reached.len()
    );
    for (library, entries) in reached {
        text.push_str(&format!("{library}\n"));
        if entries.is_empty() {
            text.push_str("  selected by an uncovered root; no declaration is loaded\n");
        }
        for chain in entries {
            let Some((declaration, requesters)) = chain.split_last() else {
                continue;
            };
            text.push_str(&format!("  {}\n", declaration_name(declaration)));
            if !requesters.is_empty() {
                let via = requesters.iter().map(declaration_name).collect::<Vec<_>>().join(" -> ");
                text.push_str(&format!("    via {via}\n"));
            }
            let origin = chain
                .first()
                .and_then(|root| origins.get(root))
                .map_or_else(|| "an unrecorded root".to_owned(), |origin| origin.render(project_root));
            text.push_str(&format!("    requested by {origin}\n"));
        }
    }
    text
}

/// A declaration's kind and canonical path, without the source offsets that change with every edit of its package.
fn declaration_name(identity: &CanonicalSymbolId) -> String {
    let origin = match &identity.origin {
        SymbolOrigin::Module(path) => path.join("::"),
        SymbolOrigin::Package { library, module_path } => {
            let mut parts = vec![format!("pub::{library}")];
            parts.extend(module_path.iter().cloned());
            parts.join("::")
        }
        SymbolOrigin::RustCrate(path) => format!("rust::{}", path.join("::")),
        SymbolOrigin::Builtin => "builtin".to_owned(),
    };
    let member = if identity.namespace == SymbolNamespace::Member {
        "member "
    } else {
        ""
    };
    format!(
        "{member}{} {origin}::{}",
        identity.kind.as_str(),
        identity.declaration_name
    )
}

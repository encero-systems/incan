//! Documentation kept apart from checked API compile facts (RFC 179).
//!
//! Installed compiler records keep docstrings out of the contract metadata a compile reads. The converter takes them
//! out of the checked API package in declaration order, each with the source anchor of the declaration or method it
//! documents; a reader that needs documentation puts them back in the same order and refuses an entry whose position
//! names another anchor.

use serde::{Deserialize, Serialize};

use super::{ApiDeclaration, ApiDocstring, CheckedApiMetadataPackage, SourceAnchor};
use crate::library_manifest::LibraryContractMetadata;

/// The documentation of one checked API declaration or method, kept apart from its compile facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiDocumentation {
    /// Position of the documented item among the package's declarations and methods, in declaration order.
    pub position: usize,
    /// Source anchor identity of the documented declaration or method.
    pub anchor: String,
    pub docstring: Option<String>,
    pub docstring_sections: Option<ApiDocstring>,
}

/// One item of a checked API package that can carry documentation: its anchor and its two documentation fields.
struct DocumentedSlot<'a> {
    anchor: &'a SourceAnchor,
    docstring: &'a mut Option<String>,
    docstring_sections: &'a mut Option<ApiDocstring>,
}

impl CheckedApiMetadataPackage {
    /// Take every docstring out of the package, leaving its compile facts; only documented items are listed.
    pub fn take_documentation(&mut self) -> Vec<ApiDocumentation> {
        documented_slots(self)
            .into_iter()
            .enumerate()
            .filter(|(_, slot)| slot.docstring.is_some() || slot.docstring_sections.is_some())
            .map(|(position, slot)| ApiDocumentation {
                position,
                anchor: slot.anchor.id.clone(),
                docstring: slot.docstring.take(),
                docstring_sections: slot.docstring_sections.take(),
            })
            .collect()
    }

    /// Put documentation taken by [`Self::take_documentation`] back, refusing an entry whose position names another
    /// anchor or lies past the package's documentable items.
    pub fn restore_documentation(&mut self, documentation: Vec<ApiDocumentation>) -> Result<(), String> {
        let mut slots = documented_slots(self);
        let documentable = slots.len();
        for entry in documentation {
            let slot = slots.get_mut(entry.position).ok_or_else(|| {
                format!(
                    "documentation for `{}` names item {} of a package with {documentable} documentable items",
                    entry.anchor, entry.position
                )
            })?;
            if slot.anchor.id != entry.anchor {
                return Err(format!(
                    "documentation for `{}` names item {}, which is `{}`",
                    entry.anchor, entry.position, slot.anchor.id
                ));
            }
            *slot.docstring = entry.docstring;
            *slot.docstring_sections = entry.docstring_sections;
        }
        Ok(())
    }
}

impl LibraryContractMetadata {
    /// Split into the compile facts and the documentation of their checked API, which compiles never read.
    pub fn split_documentation(&self) -> (Self, Vec<ApiDocumentation>) {
        let mut facts = self.clone();
        let documentation = facts
            .api
            .as_mut()
            .map(CheckedApiMetadataPackage::take_documentation)
            .unwrap_or_default();
        (facts, documentation)
    }

    /// Restore documentation split off by [`Self::split_documentation`].
    pub fn restore_documentation(&mut self, documentation: Vec<ApiDocumentation>) -> Result<(), String> {
        match self.api.as_mut() {
            Some(api) => api.restore_documentation(documentation),
            None if documentation.is_empty() => Ok(()),
            None => Err("documentation names a checked API the contract metadata does not carry".to_owned()),
        }
    }
}

/// Every documentable item of `package` in declaration order: each declaration, then its methods.
fn documented_slots(package: &mut CheckedApiMetadataPackage) -> Vec<DocumentedSlot<'_>> {
    let mut slots = Vec::new();
    for module in &mut package.modules {
        for declaration in &mut module.declarations {
            let methods = match declaration {
                ApiDeclaration::Function(item) => {
                    slots.push(slot(&item.anchor, &mut item.docstring, &mut item.docstring_sections));
                    continue;
                }
                ApiDeclaration::Model(item) => {
                    slots.push(slot(&item.anchor, &mut item.docstring, &mut item.docstring_sections));
                    &mut item.methods
                }
                ApiDeclaration::Class(item) => {
                    slots.push(slot(&item.anchor, &mut item.docstring, &mut item.docstring_sections));
                    &mut item.methods
                }
                ApiDeclaration::Trait(item) => {
                    slots.push(slot(&item.anchor, &mut item.docstring, &mut item.docstring_sections));
                    &mut item.methods
                }
                ApiDeclaration::Enum(item) => {
                    slots.push(slot(&item.anchor, &mut item.docstring, &mut item.docstring_sections));
                    &mut item.methods
                }
                ApiDeclaration::Newtype(item) => {
                    slots.push(slot(&item.anchor, &mut item.docstring, &mut item.docstring_sections));
                    &mut item.methods
                }
                ApiDeclaration::TypeAlias(_)
                | ApiDeclaration::Const(_)
                | ApiDeclaration::Static(_)
                | ApiDeclaration::Alias(_)
                | ApiDeclaration::Partial(_) => continue,
            };
            for method in methods {
                slots.push(slot(
                    &method.anchor,
                    &mut method.docstring,
                    &mut method.docstring_sections,
                ));
            }
        }
    }
    slots
}

/// One documentable item's anchor and documentation fields.
fn slot<'a>(
    anchor: &'a SourceAnchor,
    docstring: &'a mut Option<String>,
    docstring_sections: &'a mut Option<ApiDocstring>,
) -> DocumentedSlot<'a> {
    DocumentedSlot {
        anchor,
        docstring,
        docstring_sections,
    }
}

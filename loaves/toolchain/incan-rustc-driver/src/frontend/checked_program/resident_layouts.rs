//! Hand checked Rust layouts to native planning and return their exact authored occurrences to the resident pages.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::Arc;

use incan_semantics_core::SymbolOrigin;
use incan_semantics_core::body_ir::{BodyIrModule, EnumDeclaration, NominalDeclaration};

use super::RetainedNamespaceBody;

/// One authored occurrence in its original namespace order.
pub(super) struct RetainedLayout<T> {
    position: usize,
    value: LayoutValue<T>,
}

enum LayoutValue<T> {
    Prepared(Arc<str>),
    Duplicate(T),
}

/// An authored namespace's Rust layouts while the native plan has their declaration authority.
#[derive(Default)]
pub(super) struct AuthoredRustLayouts {
    models: Vec<RetainedLayout<NominalDeclaration>>,
    enums: Vec<RetainedLayout<EnumDeclaration>>,
}

impl AuthoredRustLayouts {
    /// The native declarations moved out of this namespace while planning.
    pub(super) fn len(&self) -> usize {
        self.models.len() + self.enums.len()
    }
}

/// A native layout's checked owner and storage in the planning and retained namespace inventories.
pub(super) trait RustLayout: Sized + PartialEq {
    const CONFLICT: &'static str;

    /// Select the linked item of a Rust-owned declaration.
    fn native_item(&self) -> Option<&str>;

    /// Access the module's ordered declarations of this layout kind.
    fn declarations(module: &mut BodyIrModule) -> &mut Vec<Self>;

    /// Access this layout kind's original positions and temporary custody in the namespace record.
    fn retained(namespace: &mut RetainedNamespaceBody) -> &mut Vec<RetainedLayout<Self>>;
}

impl RustLayout for NominalDeclaration {
    const CONFLICT: &'static str = "conflicting native field layouts for";

    fn native_item(&self) -> Option<&str> {
        matches!(self.canonical.origin, SymbolOrigin::RustCrate(_)).then_some(&self.native_item)
    }

    fn declarations(module: &mut BodyIrModule) -> &mut Vec<Self> {
        &mut module.nominal_declarations
    }

    fn retained(namespace: &mut RetainedNamespaceBody) -> &mut Vec<RetainedLayout<Self>> {
        &mut namespace.rust_layouts.models
    }
}

impl RustLayout for EnumDeclaration {
    const CONFLICT: &'static str = "conflicting native enum layouts for";

    fn native_item(&self) -> Option<&str> {
        matches!(self.canonical.origin, SymbolOrigin::RustCrate(_)).then_some(&self.native_item)
    }

    fn declarations(module: &mut BodyIrModule) -> &mut Vec<Self> {
        &mut module.enum_declarations
    }

    fn retained(namespace: &mut RetainedNamespaceBody) -> &mut Vec<RetainedLayout<Self>> {
        &mut namespace.rust_layouts.enums
    }
}

/// Select one equal layout per native item. Keep the first selection's address and move any later authored
/// occurrence into its own retained namespace. Runtime continuations supply no new retention entries.
pub(super) fn take_rust_layouts<T: RustLayout>(
    modules: &mut [BodyIrModule],
    authored_start: usize,
    retained: &mut [RetainedNamespaceBody],
) -> Result<Vec<T>, String> {
    let mut selected = BTreeMap::<Arc<str>, T>::new();
    let mut retained = retained.iter_mut().peekable();
    for (index, module) in modules.iter_mut().enumerate() {
        let mut saved = retained
            .next_if(|namespace| index.checked_sub(authored_start) == Some(namespace.offset))
            .map(T::retained);
        let position = Cell::new(0);
        let mut ordinal = 0;
        for declaration in T::declarations(module).extract_if(.., |candidate| {
            position.set(ordinal);
            ordinal += 1;
            candidate.native_item().is_some()
        }) {
            let item = declaration
                .native_item()
                .ok_or("selected Rust layout lost its native item")?;
            if let Some(previous) = selected.get(item) {
                if previous != &declaration {
                    return Err(format!("{} {item}", T::CONFLICT));
                }
                if let Some(saved) = &mut saved {
                    saved.push(RetainedLayout {
                        position: position.get(),
                        value: LayoutValue::Duplicate(declaration),
                    });
                }
                continue;
            }
            let item: Arc<str> = Arc::from(item);
            if let Some(saved) = &mut saved {
                saved.push(RetainedLayout {
                    position: position.get(),
                    value: LayoutValue::Prepared(Arc::clone(&item)),
                });
            }
            selected.insert(item, declaration);
        }
    }
    Ok(selected.into_values().collect())
}

/// Move just the promised layouts out of the completed dependency prefix.
fn take_prepared<'a, T: RustLayout>(
    modules: &mut [BodyIrModule],
    authored: impl Iterator<Item = &'a Arc<str>>,
) -> BTreeMap<Arc<str>, T> {
    let wanted = authored.map(|name| (name.as_ref(), name)).collect::<BTreeMap<_, _>>();
    if wanted.is_empty() {
        return BTreeMap::new();
    }
    let mut selected = BTreeMap::new();
    for module in modules {
        for declaration in T::declarations(module).extract_if(.., |value| {
            value.native_item().is_some_and(|item| wanted.contains_key(item))
        }) {
            if let Some(item) = declaration.native_item().and_then(|item| wanted.get(item)) {
                selected.entry(Arc::clone(item)).or_insert(declaration);
            }
        }
        if selected.len() == wanted.len() {
            break;
        }
    }
    selected
}

/// Merge the parked and prepared layouts into their original slots, moving the remaining declarations once.
fn restore<T>(
    declarations: &mut Vec<T>,
    prepared: &mut BTreeMap<Arc<str>, T>,
    retained: Vec<RetainedLayout<T>>,
) -> Option<()> {
    if retained.is_empty() {
        return Some(());
    }
    let mut remaining = std::mem::take(declarations).into_iter();
    let mut original = Vec::with_capacity(remaining.len() + retained.len());
    for layout in retained {
        let preceding = layout.position.checked_sub(original.len())?;
        original.extend(remaining.by_ref().take(preceding));
        if original.len() != layout.position {
            return None;
        }
        original.push(match layout.value {
            LayoutValue::Prepared(item) => prepared.remove(item.as_ref())?,
            LayoutValue::Duplicate(value) => value,
        });
    }
    original.extend(remaining);
    *declarations = original;
    Some(())
}

/// Borrow exactly the layouts that planning must return from the prepared prefix.
fn prepared_names<T>(layouts: &[RetainedLayout<T>]) -> impl Iterator<Item = &Arc<str>> {
    layouts.iter().filter_map(|layout| match &layout.value {
        LayoutValue::Prepared(name) => Some(name),
        LayoutValue::Duplicate(_) => None,
    })
}

/// The native prefix's completed layout records available to return to their original owners.
pub(super) struct ReturnedRustLayouts {
    models: BTreeMap<Arc<str>, NominalDeclaration>,
    enums: BTreeMap<Arc<str>, EnumDeclaration>,
}

impl ReturnedRustLayouts {
    /// Select the prepared declaration keys without relinquishing the original authored positions.
    pub(super) fn take(prefix: &mut [BodyIrModule], retained: &[RetainedNamespaceBody]) -> Self {
        let models = retained
            .iter()
            .flat_map(|body| prepared_names(&body.rust_layouts.models));
        let enums = retained
            .iter()
            .flat_map(|body| prepared_names(&body.rust_layouts.enums));
        Self {
            models: take_prepared(prefix, models),
            enums: take_prepared(prefix, enums),
        }
    }

    /// Return the namespace to its checked contents or leave it to the committed encoded record on a miss.
    pub(super) fn restore(&mut self, module: &mut BodyIrModule, retained: AuthoredRustLayouts) -> Option<()> {
        restore(&mut module.nominal_declarations, &mut self.models, retained.models)?;
        restore(&mut module.enum_declarations, &mut self.enums, retained.enums)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use incan_semantics_core::body_ir::native_enum_declaration;
    use incan_semantics_core::{CanonicalSymbolId, HirSourceSpan, SemanticSourceTargetKind};

    fn namespace(offset: usize) -> RetainedNamespaceBody {
        RetainedNamespaceBody {
            source: format!("src/namespace_{offset}.incn"),
            offset,
            scopes: Vec::new(),
            callables: 0..0,
            entry_union_count: None,
            rust_layouts: AuthoredRustLayouts::default(),
        }
    }

    /// Move one layout to planning and an equal duplicate to its authored owner, preserving both original layouts.
    #[test]
    fn native_layouts_return_to_their_original_namespaces_without_copying() -> Result<(), String> {
        let native = native_enum_declaration("example::Choice", Vec::new(), vec![("A".into(), Vec::new())])
            .ok_or("fixture Rust enum identity refused")?;
        let mut local = native.clone();
        local.canonical = CanonicalSymbolId::module_declaration(
            vec!["authored".into()],
            "Local",
            SemanticSourceTargetKind::Enum,
            HirSourceSpan::new(0, 1),
        );
        local.native_item.clear();
        let duplicate = native.clone();
        let first_address = native.native_item.as_ptr();
        let duplicate_address = duplicate.native_item.as_ptr();
        let mut modules = [
            BodyIrModule::declarations_only("dependency".into()),
            BodyIrModule::declarations_only("first".into()),
            BodyIrModule::declarations_only("missed".into()),
            BodyIrModule::declarations_only("second".into()),
        ];
        modules[1].enum_declarations = vec![local.clone(), native, local.clone()];
        modules[3].enum_declarations = vec![duplicate, local];
        let wanted: Vec<_> = modules.iter().map(|module| module.enum_declarations.clone()).collect();
        let mut retained = [namespace(0), namespace(2)];
        let mut selected = take_rust_layouts::<EnumDeclaration>(&mut modules, 1, &mut retained)?;
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].native_item.as_ptr(), first_address);
        // A retry sees the prepared owner in the prefix and no authored copies.
        modules[0].enum_declarations.append(&mut selected);
        let mut selected = take_rust_layouts::<EnumDeclaration>(&mut modules, 1, &mut [])?;
        modules[0].enum_declarations.append(&mut selected);
        let mut returned = ReturnedRustLayouts::take(&mut modules[..1], &retained);
        for saved in retained {
            returned
                .restore(&mut modules[saved.offset + 1], saved.rust_layouts)
                .ok_or("could not restore the exact authored layouts")?;
        }
        assert_eq!(modules[1].enum_declarations, wanted[1]);
        assert_eq!(modules[3].enum_declarations, wanted[3]);
        assert_eq!(modules[1].enum_declarations[1].native_item.as_ptr(), first_address);
        assert_eq!(modules[3].enum_declarations[0].native_item.as_ptr(), duplicate_address);
        assert!(modules[0].enum_declarations.is_empty());
        Ok(())
    }
}

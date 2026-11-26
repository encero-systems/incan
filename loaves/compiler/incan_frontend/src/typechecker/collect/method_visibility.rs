//! Scopes of the methods whose declaration does not decide where they may be called from (#2570).
//!
//! Collection gives each method the scope its declaration writes: `pub def` is public and an unmarked method is
//! private to its type. Two kinds of method take another scope. A method that implements an adopted trait's method
//! takes the trait's visibility, as a method declared in the trait does: `def describe(self) -> str` on a type adopting
//! `pub trait Describe` is public. A language hook, a method the language or generated code calls by its name, is
//! public by rule. Once a module's declarations and supertraits are collected, this pass gives both their scope, so
//! access checks, checked exports and compiled library metadata see the scope each method has.

use incan_lang::lang::conventions;
use incan_lang::lang::derives::{self, DeriveId};

use super::*;

/// A method's name and the trait it names as its target, which decide the trait method it implements.
type ImplementationKey = (String, Option<String>);

/// What a language hook depends on in the type that declares the method.
#[derive(Debug, Clone, Copy)]
pub(in crate::typechecker) struct HookOwner {
    /// The type is a newtype, whose validated constructor calls `from_underlying`.
    pub(in crate::typechecker) is_newtype: bool,
    /// The type derives `Validate`, whose generated constructor calls `validate`.
    pub(in crate::typechecker) derives_validate: bool,
}

/// The scope of a method declared in a trait with this visibility: public for a `pub` trait, and otherwise the
/// trait's module, the only module that can name the trait.
pub(in crate::typechecker) fn trait_method_scope(visibility: Visibility) -> MethodScope {
    match visibility {
        Visibility::Public => MethodScope::Public,
        Visibility::Private => MethodScope::Module,
    }
}

/// Whether the language or generated code calls this method by its name, without a trait, so it is public by rule.
///
/// This is the one place that decides the rule: a dunder method such as `__eq__`, `__str__`, `__add__` or `__iter__`,
/// the constructor hook `__incan_new`, a newtype's `from_underlying`, and the `validate` method of a type that derives
/// `Validate`.
pub(in crate::typechecker) fn is_language_hook_method(name: &str, owner: HookOwner) -> bool {
    let dunder = name.len() > 4 && name.starts_with("__") && name.ends_with("__");
    dunder
        || name == conventions::TYPE_CONSTRUCTOR_HOOK
        || (owner.is_newtype && name == conventions::NEWTYPE_FROM_UNDERLYING_METHOD)
        || (owner.derives_validate && name == conventions::VALIDATE_METHOD)
}

impl TypeChecker {
    /// Map each method name reachable through the named traits, including transitive supertrait methods (RFC 042), to
    /// the first adopted trait or supertrait that declares it, in adoption order.
    pub(in crate::typechecker) fn trait_method_origins<'a>(
        &self,
        trait_names: impl IntoIterator<Item = &'a str>,
    ) -> HashMap<String, String> {
        let mut origins = HashMap::new();
        for trait_name in trait_names {
            let supertraits = self
                .semantic_supertrait_closure(trait_name)
                .into_iter()
                .map(|(name, _)| name);
            for origin in std::iter::once(trait_name.to_string()).chain(supertraits) {
                let Some(trait_info) = self.lookup_semantic_trait_info(&origin) else {
                    continue;
                };
                for method_name in trait_info.methods.keys() {
                    origins.entry(method_name.clone()).or_insert_with(|| origin.clone());
                }
            }
        }
        origins
    }

    /// Give each language hook of the module's types the public scope, and each method that implements an adopted
    /// trait's method the scope of that trait method. Call once the declarations are collected and their supertraits
    /// resolved. Only declarations whose symbol is still that declaration are touched, and a method whose trait is not
    /// visible here keeps its own scope.
    pub(in crate::typechecker) fn settle_method_scopes(&mut self, declarations: &[Spanned<Declaration>]) {
        let local = local_traits(declarations);
        for declaration in declarations {
            let (name, traits, extends) = match &declaration.node {
                Declaration::Model(model) => (model.name.as_str(), &model.traits, None),
                Declaration::Class(class) => (class.name.as_str(), &class.traits, class.extends.as_deref()),
                Declaration::Enum(en) => (en.name.as_str(), &en.traits, None),
                Declaration::Newtype(nt) => (nt.name.as_str(), &nt.traits, None),
                _ => continue,
            };
            // A subclass also implements the traits its ancestors adopt, through the methods it inherits or overrides.
            let mut trait_names = traits.iter().map(|bound| bound.node.name.clone()).collect::<Vec<_>>();
            trait_names.extend(self.ancestor_trait_names(extends));
            let Some(symbol_id) = self.symbols.lookup(name) else {
                continue;
            };
            let Some(symbol) = self
                .symbols
                .get(symbol_id)
                .filter(|symbol| symbol.span == declaration.span)
            else {
                continue;
            };
            let Some((methods, overloads, hook_owner)) = method_tables(&symbol.kind) else {
                continue;
            };
            let origins = self.trait_method_origins(trait_names.iter().map(String::as_str));
            let mut settled: HashMap<ImplementationKey, MethodScope> = HashMap::new();
            let entries = methods.iter().chain(
                overloads
                    .iter()
                    .flat_map(|(name, group)| group.iter().map(move |method| (name, method))),
            );
            for (method_name, method) in entries {
                let key = implementation_key(method_name, method);
                if settled.contains_key(&key) {
                    continue;
                }
                let scope = if is_language_hook_method(method_name, hook_owner) {
                    Some(MethodScope::Public)
                } else {
                    self.implemented_trait_method_scope(method_name, key.1.as_deref(), &origins, &trait_names, &local)
                };
                if let Some(scope) = scope {
                    settled.insert(key, scope);
                }
            }
            if settled.is_empty() {
                continue;
            }
            let Some(symbol) = self.symbols.get_mut(symbol_id) else {
                continue;
            };
            let Some((methods, overloads)) = method_tables_mut(&mut symbol.kind) else {
                continue;
            };
            let entries = methods.iter_mut().chain(
                overloads
                    .iter_mut()
                    .flat_map(|(name, group)| group.iter_mut().map(move |method| (&*name, method))),
            );
            for (method_name, method) in entries {
                if let Some(scope) = settled.get(&implementation_key(method_name, method)) {
                    method.scope = *scope;
                }
            }
            // A method alias or partial keeps the scope of the method it targets (RFC 083).
            let targets = methods
                .iter()
                .filter(|(_, method)| method.alias_of.is_none())
                .map(|(name, method)| (name.clone(), method.scope))
                .collect::<HashMap<_, _>>();
            for method in methods.values_mut().chain(overloads.values_mut().flatten()) {
                if let Some(scope) = method.alias_of.as_ref().and_then(|target| targets.get(target)) {
                    method.scope = *scope;
                }
            }
        }
    }

    /// The scope of the adopted trait method that a method named `method_name` implements, if it implements one: the
    /// trait its target names, or else the first adopted trait or supertrait declaring that name. A trait the module
    /// declares without `pub` is found in its declarations, since a module importing this one does not collect it.
    fn implemented_trait_method_scope(
        &self,
        method_name: &str,
        target: Option<&str>,
        origins: &HashMap<String, String>,
        trait_names: &[String],
        local: &HashMap<&str, LocalTrait<'_>>,
    ) -> Option<MethodScope> {
        let trait_name = target.or_else(|| origins.get(method_name).map(String::as_str));
        let collected = trait_name
            .and_then(|trait_name| self.lookup_semantic_trait_info(trait_name))
            .and_then(|trait_info| trait_info.methods.get(method_name));
        if let Some(trait_method) = collected {
            return Some(trait_method.scope);
        }
        let declared = match target {
            Some(target) => local.get(target),
            None => trait_names.iter().find_map(|trait_name| {
                local
                    .get(trait_name.as_str())
                    .filter(|local| local.methods.contains(method_name))
            }),
        }?;
        declared
            .methods
            .contains(method_name)
            .then(|| trait_method_scope(declared.visibility))
    }

    /// The traits adopted by a class's ancestors, nearest first, starting at the class it extends.
    pub(in crate::typechecker) fn ancestor_trait_names(&self, extends: Option<&str>) -> Vec<String> {
        let mut names = Vec::new();
        let mut parent = extends.map(str::to_string);
        let mut visited = HashSet::new();
        while let Some(name) = parent.take() {
            if !visited.insert(name.clone()) {
                break;
            }
            let Some(TypeInfo::Class(info)) = self.lookup_type_info(&name) else {
                break;
            };
            names.extend(info.traits.iter().cloned());
            parent = info.extends.clone();
        }
        names
    }
}

impl TypeChecker {
    /// Give a class method that implements a Rust trait's method the public scope, the visibility of the Rust trait.
    /// Checking the class's adoption finds these, since only the Rust metadata names the trait's methods.
    pub(in crate::typechecker) fn publish_rust_trait_method(&mut self, type_name: &str, method_name: &str) {
        let Some(symbol) = self
            .symbols
            .lookup(type_name)
            .and_then(|symbol_id| self.symbols.get_mut(symbol_id))
        else {
            return;
        };
        let Some((methods, overloads)) = method_tables_mut(&mut symbol.kind) else {
            return;
        };
        let entries = methods
            .get_mut(method_name)
            .into_iter()
            .chain(overloads.get_mut(method_name).into_iter().flatten());
        for method in entries {
            method.scope = MethodScope::Public;
        }
    }
}

/// A trait declared by the module whose method scopes are settled: its visibility and method names.
struct LocalTrait<'a> {
    visibility: Visibility,
    methods: HashSet<&'a str>,
}

/// The traits a module declares, by name.
fn local_traits(declarations: &[Spanned<Declaration>]) -> HashMap<&str, LocalTrait<'_>> {
    declarations
        .iter()
        .filter_map(|declaration| match &declaration.node {
            Declaration::Trait(tr) => Some((
                tr.name.as_str(),
                LocalTrait {
                    visibility: tr.visibility,
                    methods: tr.methods.iter().map(|method| method.node.name.as_str()).collect(),
                },
            )),
            _ => None,
        })
        .collect()
}

/// Key one method entry by its name and the trait it targets.
fn implementation_key(method_name: &str, method: &MethodInfo) -> ImplementationKey {
    (
        method_name.to_string(),
        method.trait_target.as_ref().map(|target| target.name.clone()),
    )
}

/// The hook context of a collected nominal type.
pub(in crate::typechecker) fn hook_owner(info: &TypeInfo) -> HookOwner {
    let derives: &[String] = match info {
        TypeInfo::Model(info) => &info.derives,
        TypeInfo::Class(info) => &info.derives,
        TypeInfo::Enum(info) => &info.derives,
        TypeInfo::Newtype(_) | TypeInfo::Builtin | TypeInfo::TypeAlias => &[],
    };
    HookOwner {
        is_newtype: matches!(info, TypeInfo::Newtype(_)),
        derives_validate: derives
            .iter()
            .any(|derive| derives::from_str(derive) == Some(DeriveId::Validate)),
    }
}

/// The method and overload tables of a nominal type that declares methods, with its hook context.
fn method_tables(
    kind: &SymbolKind,
) -> Option<(
    &HashMap<String, MethodInfo>,
    &HashMap<String, Vec<MethodInfo>>,
    HookOwner,
)> {
    let SymbolKind::Type(info) = kind else {
        return None;
    };
    let owner = hook_owner(info);
    match info {
        TypeInfo::Model(info) => Some((&info.methods, &info.method_overloads, owner)),
        TypeInfo::Class(info) => Some((&info.methods, &info.method_overloads, owner)),
        TypeInfo::Enum(info) => Some((&info.methods, &info.method_overloads, owner)),
        TypeInfo::Newtype(info) => Some((&info.methods, &info.method_overloads, owner)),
        TypeInfo::Builtin | TypeInfo::TypeAlias => None,
    }
}

/// The mutable method and overload tables of a nominal type that declares methods.
fn method_tables_mut(
    kind: &mut SymbolKind,
) -> Option<(&mut HashMap<String, MethodInfo>, &mut HashMap<String, Vec<MethodInfo>>)> {
    match kind {
        SymbolKind::Type(TypeInfo::Model(info)) => Some((&mut info.methods, &mut info.method_overloads)),
        SymbolKind::Type(TypeInfo::Class(info)) => Some((&mut info.methods, &mut info.method_overloads)),
        SymbolKind::Type(TypeInfo::Enum(info)) => Some((&mut info.methods, &mut info.method_overloads)),
        SymbolKind::Type(TypeInfo::Newtype(info)) => Some((&mut info.methods, &mut info.method_overloads)),
        _ => None,
    }
}

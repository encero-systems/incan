//! Visibility of the methods that implement an adopted trait's method (#2570).
//!
//! A method declared in a trait takes the trait's visibility, and so does a method of a model, class, enum or newtype
//! that implements one: `def describe(self) -> str` on a type adopting `pub trait Describe` is public, while an
//! unmarked method of its own is private. Collection records each method's written visibility; once a module's
//! declarations and supertraits are collected, this pass gives each implementing method the visibility of the trait
//! method it implements, so checked exports and compiled library metadata carry the visibility the method has.

use super::*;

/// A method's name and the trait it names as its target, which decide the trait method it implements.
type ImplementationKey = (String, Option<String>);

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

    /// Give each method of the module's types that implements an adopted trait's method the visibility of that trait
    /// method. Call once the declarations are collected and their supertraits resolved. Only declarations whose symbol
    /// is still that declaration are touched, and a method whose trait is not visible here keeps its own visibility.
    pub(in crate::typechecker) fn settle_trait_implementation_visibility(
        &mut self,
        declarations: &[Spanned<Declaration>],
    ) {
        for declaration in declarations {
            let (name, traits) = match &declaration.node {
                Declaration::Model(model) => (model.name.as_str(), &model.traits),
                Declaration::Class(class) => (class.name.as_str(), &class.traits),
                Declaration::Enum(en) => (en.name.as_str(), &en.traits),
                Declaration::Newtype(nt) => (nt.name.as_str(), &nt.traits),
                _ => continue,
            };
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
            let Some((methods, overloads)) = method_tables(&symbol.kind) else {
                continue;
            };
            let origins = self.trait_method_origins(traits.iter().map(|bound| bound.node.name.as_str()));
            let mut settled: HashMap<ImplementationKey, Visibility> = HashMap::new();
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
                let trait_name = key
                    .1
                    .as_deref()
                    .or_else(|| origins.get(method_name).map(String::as_str));
                let visibility = trait_name
                    .and_then(|trait_name| self.lookup_semantic_trait_info(trait_name))
                    .and_then(|trait_info| trait_info.methods.get(method_name))
                    .map(|trait_method| trait_method.visibility);
                if let Some(visibility) = visibility {
                    settled.insert(key, visibility);
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
                if let Some(visibility) = settled.get(&implementation_key(method_name, method)) {
                    method.visibility = *visibility;
                }
            }
        }
    }
}

/// Key one method entry by its name and the trait it targets.
fn implementation_key(method_name: &str, method: &MethodInfo) -> ImplementationKey {
    (
        method_name.to_string(),
        method.trait_target.as_ref().map(|target| target.name.clone()),
    )
}

/// The method and overload tables of a nominal type that declares methods.
fn method_tables(kind: &SymbolKind) -> Option<(&HashMap<String, MethodInfo>, &HashMap<String, Vec<MethodInfo>>)> {
    match kind {
        SymbolKind::Type(TypeInfo::Model(info)) => Some((&info.methods, &info.method_overloads)),
        SymbolKind::Type(TypeInfo::Class(info)) => Some((&info.methods, &info.method_overloads)),
        SymbolKind::Type(TypeInfo::Enum(info)) => Some((&info.methods, &info.method_overloads)),
        SymbolKind::Type(TypeInfo::Newtype(info)) => Some((&info.methods, &info.method_overloads)),
        _ => None,
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

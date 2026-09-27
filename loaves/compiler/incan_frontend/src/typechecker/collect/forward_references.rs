//! Resolve references to types declared later in the same module once collection has seen every declaration (#1780).
//!
//! Collection registers declarations in source order, so a signature that names a type declared further down the file
//! is resolved before that type exists. The shared resolver keeps such a name as a `TypeVar` placeholder rather than
//! guessing, which is right while collection is still running but wrong afterwards: a method returning a model declared
//! after its owner kept the placeholder, and a call site then found no fields on it (`Type 'Made' has no field 'x'`).
//! Declaration order must not matter within a module, so once every declaration is collected this pass rewrites the
//! placeholders in the collected signatures to the types they name.
//!
//! A placeholder is rewritten only when its name is a type-position declaration of the same module whose symbol is that
//! declaration, and never where a type parameter of the enclosing declaration or method carries the same name.

use std::collections::{HashMap, HashSet};

use crate::ast::{Declaration, Spanned, Type};
use crate::resolved_type_subst::{substitute_method_info, substitute_property_info, substitute_resolved_type};
use crate::symbols::{
    FieldInfo, FunctionInfo, MethodInfo, PropertyInfo, ResolvedType, SymbolKind, TypeBoundInfo, TypeInfo, resolve_type,
};

use super::TypeChecker;

/// Placeholder names mapped to the types they name, after collection.
type ForwardTypes = HashMap<String, ResolvedType>;

/// Return the name a declaration binds in type position, if it binds one.
fn type_declaration_name(declaration: &Declaration) -> Option<&str> {
    match declaration {
        Declaration::Model(model) => Some(model.name.as_str()),
        Declaration::Class(class) => Some(class.name.as_str()),
        Declaration::Trait(tr) => Some(tr.name.as_str()),
        Declaration::Enum(en) => Some(en.name.as_str()),
        Declaration::Newtype(nt) => Some(nt.name.as_str()),
        Declaration::TypeAlias(alias) => Some(alias.name.as_str()),
        _ => None,
    }
}

/// Return the forward-type map without the names a declaration's own type parameters shadow.
fn without_type_params(forward: &ForwardTypes, type_params: &[String]) -> ForwardTypes {
    let shadowed: HashSet<&str> = type_params.iter().map(String::as_str).collect();
    forward
        .iter()
        .filter(|(name, _)| !shadowed.contains(name.as_str()))
        .map(|(name, ty)| (name.clone(), ty.clone()))
        .collect()
}

/// Rewrite placeholders in every field type.
fn resolve_fields(fields: &mut HashMap<String, FieldInfo>, forward: &ForwardTypes) {
    for field in fields.values_mut() {
        field.ty = substitute_resolved_type(&field.ty, forward);
    }
}

/// Rewrite placeholders in every computed property's return type.
fn resolve_properties(properties: &mut HashMap<String, PropertyInfo>, forward: &ForwardTypes) {
    for property in properties.values_mut() {
        *property = substitute_property_info(property, forward);
    }
}

/// Rewrite placeholders in one method signature, leaving the method's own type parameters alone.
fn resolve_method(method: &mut MethodInfo, forward: &ForwardTypes) {
    let forward = without_type_params(forward, &method.type_params);
    *method = substitute_method_info(method, &forward);
}

/// Rewrite placeholders in a nominal's method table and its overload groups.
fn resolve_methods(
    methods: &mut HashMap<String, MethodInfo>,
    method_overloads: &mut HashMap<String, Vec<MethodInfo>>,
    forward: &ForwardTypes,
) {
    for method in methods.values_mut() {
        resolve_method(method, forward);
    }
    for method in method_overloads.values_mut().flatten() {
        resolve_method(method, forward);
    }
}

/// Rewrite placeholders in the type arguments of adopted traits (`model A with Convert[B]`).
fn resolve_trait_adoptions(adoptions: &mut [TypeBoundInfo], forward: &ForwardTypes) {
    for adoption in adoptions {
        for type_arg in &mut adoption.type_args {
            *type_arg = substitute_resolved_type(type_arg, forward);
        }
    }
}

/// Rewrite placeholders in a free function's signature, leaving its own type parameters alone.
fn resolve_function(info: &mut FunctionInfo, forward: &ForwardTypes) {
    let forward = without_type_params(forward, &info.type_params);
    for param in &mut info.params {
        param.ty = substitute_resolved_type(&param.ty, &forward);
    }
    info.return_type = substitute_resolved_type(&info.return_type, &forward);
    for bound in info.type_param_bound_details.values_mut().flatten() {
        for type_arg in &mut bound.type_args {
            *type_arg = substitute_resolved_type(type_arg, &forward);
        }
    }
}

/// Rewrite placeholders throughout one collected symbol.
fn resolve_symbol_kind(kind: &mut SymbolKind, forward: &ForwardTypes) {
    match kind {
        SymbolKind::Type(TypeInfo::Model(info)) => {
            let forward = without_type_params(forward, &info.type_params);
            resolve_fields(&mut info.fields, &forward);
            resolve_properties(&mut info.properties, &forward);
            resolve_methods(&mut info.methods, &mut info.method_overloads, &forward);
            resolve_trait_adoptions(&mut info.trait_adoptions, &forward);
        }
        SymbolKind::Type(TypeInfo::Class(info)) => {
            let forward = without_type_params(forward, &info.type_params);
            resolve_fields(&mut info.fields, &forward);
            resolve_properties(&mut info.properties, &forward);
            resolve_methods(&mut info.methods, &mut info.method_overloads, &forward);
            resolve_trait_adoptions(&mut info.trait_adoptions, &forward);
        }
        SymbolKind::Type(TypeInfo::Newtype(info)) => {
            let forward = without_type_params(forward, &info.type_params);
            info.underlying = substitute_resolved_type(&info.underlying, &forward);
            resolve_methods(&mut info.methods, &mut info.method_overloads, &forward);
            resolve_trait_adoptions(&mut info.trait_adoptions, &forward);
        }
        SymbolKind::Type(TypeInfo::Enum(info)) => {
            let forward = without_type_params(forward, &info.type_params);
            for payload in info.variant_fields.values_mut().flatten() {
                *payload = substitute_resolved_type(payload, &forward);
            }
            resolve_methods(&mut info.methods, &mut info.method_overloads, &forward);
            resolve_trait_adoptions(&mut info.trait_adoptions, &forward);
        }
        SymbolKind::Trait(info) => {
            let forward = without_type_params(forward, &info.type_params);
            for method in info.methods.values_mut() {
                resolve_method(method, &forward);
            }
            resolve_properties(&mut info.properties, &forward);
            for (_, required) in &mut info.requires {
                *required = substitute_resolved_type(required, &forward);
            }
        }
        SymbolKind::Function(info) => resolve_function(info, forward),
        SymbolKind::FunctionOverloads(overloads) => {
            for overload in overloads {
                resolve_function(&mut overload.info, forward);
            }
        }
        _ => {}
    }
}

impl TypeChecker {
    /// Rewrite the forward-reference placeholders left in this module's collected signatures (#1780).
    ///
    /// Call once every declaration in `declarations` has been collected. Only declarations whose symbol is still that
    /// declaration are touched (a name the module lost to an import collision is left alone), and a placeholder is
    /// rewritten only to a type declared in `declarations` itself, so another module's type with the same name is never
    /// substituted. The checked function bindings recorded for lowering are rewritten alongside their symbols.
    pub(in crate::typechecker) fn resolve_forward_type_references(&mut self, declarations: &[Spanned<Declaration>]) {
        let forward = self.forward_type_references(declarations);
        if forward.is_empty() {
            return;
        }

        // ---- Collected symbols of this module ----
        let mut resolved_symbols = HashSet::new();
        for declaration in declarations {
            let name = match &declaration.node {
                Declaration::Function(func) => func.name.as_str(),
                other => match type_declaration_name(other) {
                    Some(name) => name,
                    None => continue,
                },
            };
            let Some(symbol_id) = self.symbols.lookup(name) else {
                continue;
            };
            let Some(symbol) = self.symbols.get(symbol_id) else {
                continue;
            };
            let owns_symbol = match &symbol.kind {
                SymbolKind::FunctionOverloads(overloads) => {
                    overloads.iter().any(|overload| overload.span == declaration.span)
                }
                _ => symbol.span == declaration.span,
            };
            if !owns_symbol || !resolved_symbols.insert(symbol_id) {
                continue;
            }
            let mut kind = symbol.kind.clone();
            resolve_symbol_kind(&mut kind, &forward);
            if let Some(symbol) = self.symbols.get_mut(symbol_id) {
                symbol.kind = kind;
            }
        }

        // ---- Function bindings recorded for lowering ----
        for declaration in declarations {
            let Declaration::Function(func) = &declaration.node else {
                continue;
            };
            let type_params: Vec<String> = func.type_params.iter().map(|param| param.name.clone()).collect();
            let forward = without_type_params(&forward, &type_params);
            let bindings = &mut self.type_info.declarations;
            let by_span = bindings
                .function_bindings_by_span
                .get_mut(&(declaration.span.start, declaration.span.end));
            let by_name = bindings.function_bindings.get_mut(&func.name);
            for binding in by_span.into_iter().chain(by_name) {
                for param in &mut binding.params {
                    param.ty = substitute_resolved_type(&param.ty, &forward);
                }
                binding.return_type = substitute_resolved_type(&binding.return_type, &forward);
            }
            if let Some(overloads) = bindings.function_overloads.get_mut(&func.name) {
                for overload in overloads
                    .iter_mut()
                    .filter(|overload| overload.span == declaration.span)
                {
                    resolve_function(&mut overload.info, &forward);
                }
            }
        }
    }

    /// Map every type-position declaration of this module that collection has registered to the type its name
    /// resolves to now.
    ///
    /// A declaration counts only when the symbol its name resolves to is that declaration; a name whose resolution is
    /// still a placeholder is left out.
    fn forward_type_references(&self, declarations: &[Spanned<Declaration>]) -> ForwardTypes {
        let mut forward = ForwardTypes::new();
        for declaration in declarations {
            let Some(name) = type_declaration_name(&declaration.node) else {
                continue;
            };
            let declared_here = self
                .symbols
                .lookup(name)
                .and_then(|symbol_id| self.symbols.get(symbol_id))
                .is_some_and(|symbol| symbol.span == declaration.span);
            if !declared_here {
                continue;
            }
            let resolved = self.expand_type_aliases(resolve_type(&Type::Simple(name.to_string()), &self.symbols));
            if resolved != ResolvedType::TypeVar(name.to_string()) {
                forward.insert(name.to_string(), resolved);
            }
        }
        forward
    }
}

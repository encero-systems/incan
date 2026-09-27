//! Refuse a generic declaration that instantiates a bounded model, class or enum with a type parameter lacking the
//! bound (#1280).
//!
//! `model Stream[R with Clone]` declares that every `Stream[...]` needs a `Clone` argument, and the generated type
//! carries that requirement wherever it is named. A generic declaration that names `Stream[T]` for its own `T` must
//! therefore declare `T with Clone` too; when it did not, the checker accepted it and the build refused it. The
//! requirement is the model's own declaration, so the checker refuses the use and names the bound to write, rather than
//! inferring a bound the author did not state.
//!
//! The use is refused wherever the checker sees the type: a function or method parameter or return type, a field of a
//! generic model or class, a variant payload of a generic enum, the underlying type of a generic newtype, and the type
//! of any expression in a function or method body (a construction, a local, a call result). Trait default methods are
//! not judged: their bodies are completed per adopter, where the adopter's own declaration supplies the bound.
//!
//! The declared bounds are recorded for the models, classes, enums and newtypes the checked module declares, for those
//! of the source dependency modules it imports, for the standard-library models and classes it imports (read from the
//! checked API a compiled standard-library provider publishes, or from the library's source when no provider serves
//! the module), and for the models and classes of compiled libraries it imports (read from their manifests). They are
//! keyed by the declaration a spelling's binding names, not by the spelling: two modules may each declare a `Node`.
//! Records and lookups compute that key from the same identity: the binding in the module being checked
//! ([`TypeChecker::nominal_declaration_key`]), or the provider identity that binding carries.

use std::collections::HashMap;

use crate::api_metadata::ApiDeclaration;
use crate::ast::{ClassDecl, Declaration, EnumDecl, FieldDecl, ModelDecl, NewtypeDecl, Span, Spanned, TypeParam};
use crate::diagnostics::errors;
use crate::library_manifest::TypeParamExport;
use crate::symbols::{ResolvedType, TypeBoundInfo, TypeInfo, resolve_type};
use incan_semantics_core::{CanonicalSymbolId, SymbolOrigin};

use super::TypeChecker;

/// The declared bounds of one nominal's type parameters, in declaration order: `(parameter, bounds)`.
pub(in crate::typechecker) type NominalTypeParamBounds = Vec<(String, Vec<TypeBoundInfo>)>;

/// The declaration a nominal spelling names, as the key its declared bounds are recorded under.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(in crate::typechecker) enum NominalDeclarationKey {
    /// A binding with a canonical identity: the module or library that declares it, and its declaration name.
    Declared(SymbolOrigin, String),
    /// A binding without a canonical identity: the path of the module that binds it, and the name it binds.
    Binding(Vec<String>, String),
}

/// The declared bounds of every bounded nominal visible to the checked module, by declaration.
pub(in crate::typechecker) type NominalBoundTable = HashMap<NominalDeclarationKey, NominalTypeParamBounds>;

/// Return the declaration key of one canonical identity.
fn declaration_key(identity: &CanonicalSymbolId) -> NominalDeclarationKey {
    NominalDeclarationKey::Declared(identity.origin.clone(), identity.declaration_name.clone())
}

/// Convert the exported type parameters of a compiled model or class into their plain trait bounds.
///
/// A library trait's recorded module path need not match the path the consumer imports it under, so each bound is
/// recorded by its declaration name alone; the check accepts a consumer bound of that declaration name.
fn exported_type_param_bounds(type_params: &[TypeParamExport]) -> NominalTypeParamBounds {
    type_params
        .iter()
        .map(|param| {
            let bounds = param
                .bounds
                .iter()
                .filter(|bound| bound.type_args.is_empty())
                .map(|bound| {
                    let declaration = bound
                        .source_name
                        .clone()
                        .unwrap_or_else(|| bound.name.rsplit('.').next().unwrap_or(&bound.name).to_string());
                    builtin_bound(&declaration)
                })
                .collect();
            (param.name.clone(), bounds)
        })
        .collect()
}

/// One type argument that lacks a bound its position declares.
struct MissingNominalBound {
    type_name: String,
    type_param: String,
    argument: String,
    bound: String,
}

/// Build the bound record of a trait known by its declaration name alone, as a consumer's own bound on a builtin trait
/// (`Clone`, `Eq`, `Ord`, ...) resolves.
pub(in crate::typechecker) fn builtin_bound(name: &str) -> TypeBoundInfo {
    TypeBoundInfo {
        name: name.to_string(),
        source_name: None,
        type_args: Vec::new(),
        module_path: None,
        implementation_type_params: Vec::new(),
    }
}

/// Return the type parameters of a declaration that can bound them, with its name.
fn bounded_declaration(declaration: &Declaration) -> Option<(&str, &[TypeParam])> {
    match declaration {
        Declaration::Model(model) => Some((model.name.as_str(), model.type_params.as_slice())),
        Declaration::Class(class) => Some((class.name.as_str(), class.type_params.as_slice())),
        Declaration::Enum(en) => Some((en.name.as_str(), en.type_params.as_slice())),
        Declaration::Newtype(nt) => Some((nt.name.as_str(), nt.type_params.as_slice())),
        _ => None,
    }
}

impl TypeChecker {
    /// Record the declared type-parameter bounds of the nominals `declarations` defines (#1280).
    ///
    /// Call after collection, so a bound naming a trait declared further down the module resolves. Only plain trait
    /// bounds are recorded: a bound with type arguments names the declaration's own parameters and is left to the
    /// checks that own it. A declaration whose name is bound to something else (an import collision) is skipped, and a
    /// declaration without bounds clears any entry it had, so a table reused across checks never keeps a bound the
    /// current declaration does not have.
    pub(in crate::typechecker) fn record_local_nominal_type_param_bounds(
        &mut self,
        declarations: &[Spanned<Declaration>],
    ) {
        for declaration in declarations {
            let Some((name, type_params)) = bounded_declaration(&declaration.node) else {
                continue;
            };
            let declared_here = self
                .lookup_symbol(name)
                .is_some_and(|symbol| symbol.span == declaration.span);
            if !declared_here {
                continue;
            }
            let key = self.nominal_declaration_key(name);
            let bounds = self.declared_nominal_type_param_bounds(type_params);
            self.record_nominal_type_param_bounds(key, bounds);
        }
    }

    /// Store one declaration's bounds, or clear its entry when none of its type parameters declares a bound.
    fn record_nominal_type_param_bounds(&mut self, key: NominalDeclarationKey, bounds: NominalTypeParamBounds) {
        if bounds.iter().any(|(_, bounds)| !bounds.is_empty()) {
            self.nominal_type_param_bounds.insert(key, bounds);
        } else {
            self.nominal_type_param_bounds.remove(&key);
        }
    }

    /// Return the declaration a nominal spelling names in the module being checked.
    ///
    /// The key is the canonical identity the spelling's module-level binding carries, or else the identity a compiled
    /// library admitted for that spelling. A binding without either is keyed by the binding module and the spelling,
    /// which only that module's own records and lookups produce.
    fn nominal_declaration_key(&self, spelling: &str) -> NominalDeclarationKey {
        self.symbols
            .module_type_identity(spelling)
            .or_else(|| {
                self.public_library_type_identities
                    .get(spelling)
                    .and_then(|binding| binding.canonical.as_ref())
            })
            .map_or_else(
                || {
                    let module = self.current_module_path.clone().unwrap_or_default();
                    NominalDeclarationKey::Binding(module, spelling.to_string())
                },
                declaration_key,
            )
    }

    /// Record the plain trait bounds a compiled library's model or class declares, under the declaration the import
    /// just bound `local_name` to.
    pub(in crate::typechecker) fn record_manifest_type_param_bounds(
        &mut self,
        local_name: &str,
        type_params: &[TypeParamExport],
    ) {
        if type_params.is_empty() {
            return;
        }
        let key = self.nominal_declaration_key(local_name);
        self.record_nominal_type_param_bounds(key, exported_type_param_bounds(type_params));
    }

    /// Record the plain trait bounds a compiled standard-library provider publishes for one of its models or classes,
    /// under the canonical identity its identity graph gives the declaration.
    ///
    /// An import from a module such a provider serves binds the provider's member with that identity, so the check
    /// finds the bounds without the import recording them. A declaration without an identity is not recorded.
    pub(in crate::typechecker) fn record_provider_type_param_bounds(
        &mut self,
        declaration: &ApiDeclaration,
        identity: Option<&CanonicalSymbolId>,
    ) {
        let type_params = match declaration {
            ApiDeclaration::Model(model) => &model.type_params,
            ApiDeclaration::Class(class) => &class.type_params,
            _ => return,
        };
        if let Some(identity) = identity.filter(|_| !type_params.is_empty()) {
            self.record_nominal_type_param_bounds(declaration_key(identity), exported_type_param_bounds(type_params));
        }
    }

    /// Record the declared bounds of an imported standard-library nominal, read from its source, under the declaration
    /// the import just bound `local_name` to.
    ///
    /// The binding's identity is a compiled standard-library provider's when one serves the module, and the source
    /// declaration's otherwise; keying by the binding keeps the record where the lookup looks in both cases.
    pub(in crate::typechecker) fn record_imported_stdlib_type_param_bounds(
        &mut self,
        module_path: &[String],
        name: &str,
        local_name: &str,
    ) {
        let bounds = self.stdlib_cache.lookup_type_param_bounds(module_path, name);
        let key = self.nominal_declaration_key(local_name);
        self.record_nominal_type_param_bounds(key, bounds);
    }

    /// Resolve a nominal's declared plain trait bounds the way a generic callable's active bounds are resolved, so the
    /// two compare by the same trait identity.
    fn declared_nominal_type_param_bounds(&mut self, type_params: &[TypeParam]) -> NominalTypeParamBounds {
        type_params
            .iter()
            .map(|param| {
                let bounds = param
                    .bounds
                    .iter()
                    .filter(|bound| bound.type_args.is_empty())
                    .map(|bound| TypeBoundInfo {
                        name: self.resolve_trait_bound_name(&bound.name, Span::default()),
                        source_name: self.trait_bound_source_name(&bound.name),
                        type_args: Vec::new(),
                        module_path: self.trait_bound_module_path(&bound.name),
                        implementation_type_params: Vec::new(),
                    })
                    .collect();
                (param.name.clone(), bounds)
            })
            .collect()
    }

    /// Refuse every place in a signature type where a bounded nominal receives a type parameter of the enclosing
    /// callable or owner that does not declare the bound (#1280).
    ///
    /// Only type parameters active in the declaration being checked are judged; a concrete argument, `Self`, or a
    /// type the checker cannot see into is left to the checks and the build that own it. `span` is the annotation's.
    pub(in crate::typechecker) fn refuse_unbounded_nominal_type_arguments(&mut self, ty: &ResolvedType, span: Span) {
        if self.nominal_type_param_bounds.is_empty() {
            return;
        }
        let mut missing = Vec::new();
        self.collect_unbounded_nominal_type_arguments(ty, &mut missing);
        for missing in missing {
            self.errors.push(errors::nominal_type_argument_missing_bound(
                &missing.type_name,
                &missing.type_param,
                &missing.argument,
                &missing.bound,
                span,
            ));
        }
    }

    /// Refuse the type of one expression in a function or method body when it names a bounded nominal with an
    /// unbounded type parameter (#1280).
    ///
    /// Each expression that shows a gap reports it at its own span; an expression checked more than once reports it
    /// once. The errors themselves are the record, so a speculative check whose errors are discarded does not suppress
    /// the real report.
    pub(in crate::typechecker) fn refuse_unbounded_nominal_expression_type(&mut self, ty: &ResolvedType, span: Span) {
        if self.nominal_type_param_bounds.is_empty()
            || self.current_type_param_bound_details.is_empty()
            || self.current_trait_name.is_some()
        {
            return;
        }
        let mut missing = Vec::new();
        self.collect_unbounded_nominal_type_arguments(ty, &mut missing);
        for missing in missing {
            let error = errors::nominal_type_argument_missing_bound(
                &missing.type_name,
                &missing.type_param,
                &missing.argument,
                &missing.bound,
                span,
            );
            if !self
                .errors
                .iter()
                .any(|existing| existing.span == error.span && existing.message == error.message)
            {
                self.errors.push(error);
            }
        }
    }

    /// Refuse the field types of a generic model that name a bounded nominal with an unbounded owner parameter.
    pub(in crate::typechecker) fn refuse_unbounded_model_members(&mut self, model: &ModelDecl) {
        let members = self.field_member_types(&model.name, &model.fields);
        self.refuse_unbounded_member_types(&model.type_params, members);
    }

    /// Refuse the field types of a generic class that name a bounded nominal with an unbounded owner parameter.
    pub(in crate::typechecker) fn refuse_unbounded_class_members(&mut self, class: &ClassDecl) {
        let members = self.field_member_types(&class.name, &class.fields);
        self.refuse_unbounded_member_types(&class.type_params, members);
    }

    /// Refuse the variant payloads of a generic enum that name a bounded nominal with an unbounded owner parameter.
    pub(in crate::typechecker) fn refuse_unbounded_enum_members(&mut self, en: &EnumDecl) {
        if en.type_params.is_empty() || self.nominal_type_param_bounds.is_empty() {
            return;
        }
        let payloads = match self.lookup_type_info(&en.name) {
            Some(TypeInfo::Enum(info)) => info.variant_fields.clone(),
            _ => return,
        };
        let members = en
            .variants
            .iter()
            .filter_map(|variant| {
                let types = payloads.get(&variant.node.name)?;
                Some(
                    types
                        .iter()
                        .cloned()
                        .zip(variant.node.fields.iter().map(|field| field.span))
                        .collect::<Vec<_>>(),
                )
            })
            .flatten()
            .collect();
        self.refuse_unbounded_member_types(&en.type_params, members);
    }

    /// Refuse the underlying type of a generic newtype that names a bounded nominal with an unbounded parameter.
    pub(in crate::typechecker) fn refuse_unbounded_newtype_members(&mut self, nt: &NewtypeDecl) {
        if nt.type_params.is_empty() || self.nominal_type_param_bounds.is_empty() {
            return;
        }
        let underlying = match self.lookup_type_info(&nt.name) {
            Some(TypeInfo::Newtype(info)) => info.underlying.clone(),
            _ => return,
        };
        self.refuse_unbounded_member_types(&nt.type_params, vec![(underlying, nt.underlying.span)]);
    }

    /// Pair a model's or class's collected field types with the spans of their annotations.
    fn field_member_types(&self, owner: &str, declared: &[Spanned<FieldDecl>]) -> Vec<(ResolvedType, Span)> {
        let fields = match self.lookup_type_info(owner) {
            Some(TypeInfo::Model(info)) => &info.fields,
            Some(TypeInfo::Class(info)) => &info.fields,
            _ => return Vec::new(),
        };
        declared
            .iter()
            .filter_map(|field| Some((fields.get(&field.node.name)?.ty.clone(), field.node.ty.span)))
            .collect()
    }

    /// Judge member types with the owner's type parameters and their declared bounds active.
    fn refuse_unbounded_member_types(&mut self, type_params: &[TypeParam], members: Vec<(ResolvedType, Span)>) {
        if type_params.is_empty() || members.is_empty() || self.nominal_type_param_bounds.is_empty() {
            return;
        }
        let active = self.owner_bound_frame(type_params);
        self.current_type_param_bound_details.push(active);
        for (ty, span) in members {
            self.refuse_unbounded_nominal_type_arguments(&ty, span);
        }
        self.current_type_param_bound_details.pop();
    }

    /// Build the active-bound frame of a nominal's own type parameters for judging its member types.
    ///
    /// Member types are judged before the body check brings the parameters into scope, so a bound's type arguments
    /// are resolved without validation here: an unvalidated parameter name stays a placeholder instead of being
    /// reported as unknown, and the names themselves are validated by the declaration's own check.
    fn owner_bound_frame(&mut self, type_params: &[TypeParam]) -> HashMap<String, Vec<TypeBoundInfo>> {
        type_params
            .iter()
            .map(|param| {
                let bounds = param
                    .bounds
                    .iter()
                    .map(|bound| TypeBoundInfo {
                        name: self.resolve_trait_bound_name(&bound.name, Span::default()),
                        source_name: self.trait_bound_source_name(&bound.name),
                        type_args: bound
                            .type_args
                            .iter()
                            .map(|arg| resolve_type(&arg.node, &self.symbols))
                            .collect(),
                        module_path: self.trait_bound_module_path(&bound.name),
                        implementation_type_params: Vec::new(),
                    })
                    .collect();
                (param.name.clone(), bounds)
            })
            .collect()
    }

    /// Walk a type, collecting each type-parameter argument that lacks a bound its nominal position declares.
    fn collect_unbounded_nominal_type_arguments(&self, ty: &ResolvedType, missing: &mut Vec<MissingNominalBound>) {
        match ty {
            ResolvedType::Generic(type_name, args) => {
                if let Some(declared) = self.declared_nominal_bounds(type_name) {
                    let bindings: HashMap<String, ResolvedType> = declared
                        .iter()
                        .map(|(param, _)| param.clone())
                        .zip(args.iter().cloned())
                        .collect();
                    for ((param, bounds), arg) in declared.iter().zip(args) {
                        let Some(argument) = self.active_type_param_name(arg) else {
                            continue;
                        };
                        for bound in bounds {
                            if !self.active_type_param_satisfies_bound_info(argument, bound, &bindings)
                                && !self.active_type_param_declares_same_trait(argument, bound)
                            {
                                missing.push(MissingNominalBound {
                                    type_name: type_name.clone(),
                                    type_param: param.clone(),
                                    argument: argument.to_string(),
                                    bound: Self::type_bound_source_name(bound).to_string(),
                                });
                            }
                        }
                    }
                }
                for arg in args {
                    self.collect_unbounded_nominal_type_arguments(arg, missing);
                }
            }
            ResolvedType::Function(params, ret) => {
                for param in params {
                    self.collect_unbounded_nominal_type_arguments(&param.ty, missing);
                }
                self.collect_unbounded_nominal_type_arguments(ret, missing);
            }
            ResolvedType::Tuple(items) => {
                for item in items {
                    self.collect_unbounded_nominal_type_arguments(item, missing);
                }
            }
            ResolvedType::FrozenList(inner)
            | ResolvedType::FrozenSet(inner)
            | ResolvedType::TypeToken(inner)
            | ResolvedType::Ref(inner)
            | ResolvedType::RefMut(inner) => self.collect_unbounded_nominal_type_arguments(inner, missing),
            ResolvedType::FrozenDict(key, value) => {
                self.collect_unbounded_nominal_type_arguments(key, missing);
                self.collect_unbounded_nominal_type_arguments(value, missing);
            }
            _ => {}
        }
    }

    /// Return the declared bounds recorded for the declaration a nominal spelling names; an import alias carries the
    /// identity of the declaration it imports.
    fn declared_nominal_bounds(&self, type_name: &str) -> Option<&NominalTypeParamBounds> {
        self.nominal_type_param_bounds
            .get(&self.nominal_declaration_key(type_name))
    }

    /// Whether an active type parameter declares a bound on the required trait's declaration name, or `Copy` where
    /// `Clone` is required (every `Copy` type is `Clone`).
    ///
    /// A bound recorded from another module's declaration carries that module's spelling of the trait; the consumer's
    /// own bound on the same trait may resolve through a different import path, so the declaration name decides.
    fn active_type_param_declares_same_trait(&self, placeholder: &str, required: &TypeBoundInfo) -> bool {
        let required_name = Self::type_bound_source_name(required);
        self.current_type_param_bound_details
            .iter()
            .rev()
            .find_map(|frame| frame.get(placeholder))
            .is_some_and(|bounds| {
                bounds.iter().any(|bound| {
                    let name = Self::type_bound_source_name(bound);
                    name == required_name || (required_name == "Clone" && name == "Copy")
                })
            })
    }
}

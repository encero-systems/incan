//! Lowering for the `std.serde.json` protocol traits (`Serialize`, `Deserialize`) beside their Rust serde capability.
//!
//! The stdlib traits declare `to_json` and `from_json`; the serde capability (`serde::Serialize`,
//! `serde::de::DeserializeOwned`) is what `json_stringify`, a derive over a generic parameter and the traits' own
//! default bodies compile against. The stdlib traits do not name the capability as a supertrait, so a type parameter
//! bound, a trait-typed parameter and a declared trait-typed return on one of them carry both, and a newtype that
//! derives one of them implements the stdlib trait beside its serde derive, as a model does (#1820).

use incan_frontend::ast::{self, Spanned};
use incan_lang::lang::rust_keywords;
use incan_lang::lang::stdlib::{self, StdlibJsonTraitId};
use incan_lang::lang::trait_bounds::{self, TraitBoundId};

use super::super::super::IrProgram;
use super::super::super::decl::{IrDeclKind, IrFunction, IrImpl, IrTraitBound};
use super::super::super::expr::{IrExprKind, TypedExpr};
use super::super::super::types::IrType;
use super::super::AstLowering;

/// The generic parameters a method's return-position trait type captures, as the Rust emitter lists them for a method
/// of a trait or an impl block.
struct MethodReturnCaptures<'a> {
    /// Whether the method belongs to a trait declaration, whose returns also capture `Self`.
    owner_is_trait: bool,
    /// The type parameters of the owning trait or impl block, in declaration order.
    owner_type_params: &'a [String],
}

impl AstLowering {
    /// Return the Rust serde capability that a requirement on a `std.serde.json` protocol trait carries beside the
    /// trait, keyed on the spelling's checked identity.
    ///
    /// The identity is resolved by [`Self::canonical_trait_identity`], which follows the bare import, an alias and a
    /// name qualified by one imported module (`json.Serialize`) to the stdlib declaration; a trait that merely shares
    /// the name carries nothing, and so does a spelling that identity resolution does not follow, such as
    /// `serde.json.Serialize` after `from std import serde`. The path is absolute (`::serde::Serialize`) so a module
    /// that binds `serde` to the stdlib namespace cannot shadow the crate.
    pub(in crate::lower) fn json_protocol_capability_bound(&self, visible_name: &str) -> Option<IrTraitBound> {
        let capability = match self.stdlib_json_protocol_for_adopted_trait(visible_name)? {
            StdlibJsonTraitId::Serialize => TraitBoundId::Serialize,
            StdlibJsonTraitId::Deserialize => TraitBoundId::Deserialize,
        };
        let path = trait_bounds::rust_path(capability)?;
        Some(IrTraitBound::with_type_args_classified(format!("::{path}"), Vec::new()))
    }

    /// Return the `std.serde.json` protocol traits a newtype derives, as trait impl targets.
    ///
    /// Each target is one protocol trait named in the newtype's `@derive(...)`, directly or through a module derive
    /// such as `@derive(json)`, spelled as the derive resolves it; its impl gives the newtype the trait's methods, as
    /// the same derive gives a model. Traits outside `std.serde.json` are not returned. A trait both derived and
    /// adopted with `with` is refused by the checker, so no target duplicates an adoption's impl.
    pub(in crate::lower) fn derived_json_protocol_impl_targets(
        &mut self,
        decorators: &[Spanned<ast::Decorator>],
    ) -> Vec<(String, Vec<IrType>)> {
        self.derive_trait_impl_targets(decorators)
            .into_iter()
            .filter(|(trait_name, _)| self.stdlib_json_protocol_for_adopted_trait(trait_name).is_some())
            .collect()
    }

    /// Require the serde capability of `trait_name` on every type parameter of a derived protocol impl.
    ///
    /// The impl's default body serializes or parses the whole value, which a generic newtype supports only when its
    /// parameters do (the serde derive bounds each parameter the same way), so the impl holds exactly for those
    /// instantiations.
    pub(in crate::lower) fn require_json_protocol_capability_on_impl_params(
        &self,
        impl_block: &mut IrImpl,
        trait_name: &str,
    ) {
        let Some(capability) = self.json_protocol_capability_bound(trait_name) else {
            return;
        };
        for type_param in &mut impl_block.type_params {
            if !type_param.bounds.contains(&capability) {
                type_param.bounds.push(capability.clone());
            }
        }
    }

    /// Give every declared return of a `std.serde.json` protocol trait type the serde capability too.
    ///
    /// A return-position trait type lowers to `IrType::ImplTrait`, which holds a single bound, so `def make() ->
    /// Serialize` would return `impl Serialize` and `json_stringify(make())` would not compile. After the program is
    /// lowered, each such return of a free function, an impl method or a trait method is spelled as the exact Rust
    /// type `impl <stdlib trait> + <serde capability>` instead. The stdlib trait is named by the absolute path its
    /// method dispatch uses, so the spelling depends on no import. A method's return also carries the same
    /// `use<...>` capture list the emitter writes for a method's `ImplTrait` return: `Self` for a trait method, then
    /// the owner's and the method's type parameters.
    pub(in crate::lower) fn attach_json_protocol_capability_to_trait_returns(&self, program: &mut IrProgram) {
        for decl in &mut program.declarations {
            match &mut decl.kind {
                IrDeclKind::Function(function) => self.attach_json_protocol_capability_to_return(function, None),
                IrDeclKind::Impl(impl_block) => {
                    let owner_type_params = impl_block
                        .type_params
                        .iter()
                        .map(|param| param.name.clone())
                        .collect::<Vec<_>>();
                    for method in &mut impl_block.methods {
                        self.attach_json_protocol_capability_to_return(
                            method,
                            Some(MethodReturnCaptures {
                                owner_is_trait: false,
                                owner_type_params: &owner_type_params,
                            }),
                        );
                    }
                }
                IrDeclKind::Trait(trait_decl) => {
                    let owner_type_params = trait_decl
                        .type_params
                        .iter()
                        .map(|param| param.name.clone())
                        .collect::<Vec<_>>();
                    for method in &mut trait_decl.methods {
                        self.attach_json_protocol_capability_to_return(
                            method,
                            Some(MethodReturnCaptures {
                                owner_is_trait: true,
                                owner_type_params: &owner_type_params,
                            }),
                        );
                    }
                }
                _ => {}
            }
        }
    }

    /// Rewrite one callable's `ImplTrait` return on a `std.serde.json` protocol trait into the two-bound Rust type.
    ///
    /// `captures` is `None` for a free function, whose return the emitter writes without a capture list.
    fn attach_json_protocol_capability_to_return(
        &self,
        function: &mut IrFunction,
        captures: Option<MethodReturnCaptures<'_>>,
    ) {
        let IrType::ImplTrait(bound) = &function.return_type else {
            return;
        };
        if !bound.type_args.is_empty() || !bound.assoc_types.is_empty() {
            return;
        }
        let Some(capability) = self.json_protocol_capability_bound(&bound.trait_path) else {
            return;
        };
        let (_, Some(declaration_name)) = self.canonical_trait_identity(&bound.trait_path) else {
            return;
        };
        let json_module = [stdlib::STDLIB_ROOT, stdlib::STDLIB_SERDE, stdlib::STDLIB_JSON].map(String::from);
        let no_receiver = TypedExpr::new(IrExprKind::Unit, IrType::Unknown);
        let protocol_path = self.lower_stdlib_trait_dispatch_path(&json_module, &declaration_name, &no_receiver);
        let mut display = format!("impl {protocol_path} + {}", capability.trait_path);
        if let Some(captures) = captures {
            let mut names = Vec::new();
            if captures.owner_is_trait {
                names.push("Self".to_string());
            }
            let method_type_params = function.type_params.iter().map(|param| &param.name);
            for name in captures.owner_type_params.iter().chain(method_type_params) {
                let spelled = if rust_keywords::is_keyword(name) {
                    format!("r#{name}")
                } else {
                    name.clone()
                };
                if !names.contains(&spelled) {
                    names.push(spelled);
                }
            }
            display.push_str(&format!(" + use<{}>", names.join(", ")));
        }
        function.return_type = IrType::RustDisplay(display);
    }
}

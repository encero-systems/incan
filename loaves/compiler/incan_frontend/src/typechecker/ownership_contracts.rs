//! Recognition of RFC 164 contracts before their executable ownership stages are enabled.
//!
//! Source parsing and formatting preserve declarations. Every resolved prelude mode and result-source clause is
//! refused before native lowering until checked loan and transfer facts can implement it without repair copies.

use incan_lang::lang::types::ownership::{self, OwnershipMode};
use incan_semantics_core::SymbolOrigin;

mod checked;

use super::TypeChecker;
use crate::ast::{AssignmentStmt, BindingKind, Expr, Span, Spanned, Type};
use crate::diagnostics::CompileError;
use crate::symbols::{ResolvedType, TypeInfo};

impl TypeChecker {
    /// Warn when a new local copies a place that a place binding borrows, without changing runtime behavior or
    /// inferring a repair clone.
    ///
    /// An annotation does not opt out. Calls, constructed values, computed properties and explicit clone results are
    /// not storage places; reassignment is handled before this new-binding hook.
    pub(super) fn warn_place_binding_migration(&mut self, assignment: &AssignmentStmt) {
        if assignment.binding == BindingKind::Reassign || !self.migration_place(&assignment.value) {
            return;
        }
        let borrow = if assignment.binding == BindingKind::Mutable {
            "with write-through replacement"
        } else {
            "read-only"
        };
        self.warnings.push(CompileError::warning(
            format!("Local '{}' copies a place that a place binding borrows {borrow}", assignment.name),
            assignment.value.span,
        ).with_stable_code("INCAN-T0125")
            .with_note("This declaration receives an independent value; a type annotation does not request a copy")
            .with_hint("Use an explicit clone for an independent snapshot, or preserve the source through the borrow's last use"));
    }

    /// Admit only existing storage roots and known projections, excluding property calls and temporary owners.
    fn migration_place(&self, value: &Spanned<Expr>) -> bool {
        match &value.node {
            Expr::Ident(name) => self.lookup_variable_info_in_scope_chain(name).is_some(),
            Expr::Paren(inner) => self.migration_place(inner),
            Expr::Field(base, field) if self.migration_place(base) => {
                if self
                    .type_info
                    .resolved_identity(value.span)
                    .is_some_and(|identity| identity.kind == incan_semantics_core::SemanticSourceTargetKind::Field)
                {
                    return true;
                }
                let name = match self.type_info.expr_type(base.span) {
                    Some(ResolvedType::Named(name) | ResolvedType::Generic(name, _)) => name,
                    _ => return false,
                };
                match self.lookup_type_info(name) {
                    Some(TypeInfo::Model(model)) => model.fields.contains_key(field),
                    Some(TypeInfo::Class(class)) => class.fields.contains_key(field),
                    _ => false,
                }
            }
            Expr::Index(base, _) if self.migration_place(base) => matches!(self.type_info.expr_type(base.span),
                Some(ResolvedType::Generic(name, _)) if name == "List" || name == "Dict"),
            _ => false,
        }
    }

    /// Resolve a mode only when ordinary lookup selects its canonical builtin declaration.
    pub(super) fn explicit_ownership_mode(&self, name: &str) -> Option<OwnershipMode> {
        let symbol = self
            .symbols
            .lookup(name)
            .or_else(|| ownership::from_str(name).and_then(|mode| self.symbols.lookup(mode.as_str())))?;
        let identity = self.symbols.identity_of(symbol)?;
        if identity.origin != SymbolOrigin::Builtin {
            return None;
        }
        ownership::from_str(&identity.declaration_name)
    }

    /// Keep prelude access contracts from becoming accidental runtime wrapper constructors.
    pub(super) fn refuse_explicit_ownership_constructor(&mut self, name: &str, span: Span) -> bool {
        let Some(mode) = self.explicit_ownership_mode(name) else {
            return false;
        };
        self.errors.push(
            CompileError::type_error(
                format!(
                    "'{}' is an ownership annotation, not a runtime constructor",
                    mode.as_str()
                ),
                span,
            )
            .with_stable_code("INCAN-T0123")
            .with_hint("Write the contract in a type annotation instead of wrapping the value in a call"),
        );
        true
    }

    /// Refuse explicit mode annotations at every nesting depth, including aliases, fields and callable signatures.
    ///
    /// The annotation retains its generic shape; this diagnostic prevents interpreting that shape as a value wrapper
    /// or turning it into a native reference before the contract's executable semantics exist.
    pub(super) fn refuse_unimplemented_ownership_annotation(&mut self, annotation: &Spanned<Type>) {
        match &annotation.node {
            Type::Simple(name) | Type::Generic(name, _) => {
                if let Some(mode) = self.explicit_ownership_mode(name) {
                    let valid_arity = matches!(&annotation.node, Type::Generic(_, arguments) if arguments.len() == 1);
                    let message = if valid_arity {
                        format!(
                            "Explicit ownership contract '{}[T]' cannot be proved at this boundary",
                            mode.as_str()
                        )
                    } else {
                        format!(
                            "Ownership contract '{}' requires exactly one type argument",
                            mode.as_str()
                        )
                    };
                    self.errors.push(CompileError::type_error(message, annotation.span)
                        .with_stable_code("INCAN-T0123")
                        .with_note("This compiler refuses the contract rather than compiling it with implicit copy or clone semantics"));
                }
                if let Type::Generic(_, arguments) = &annotation.node {
                    for argument in arguments {
                        self.refuse_unimplemented_ownership_annotation(argument);
                    }
                }
            }
            Type::DottedGeneric(_, arguments) | Type::Tuple(arguments) => {
                for argument in arguments {
                    self.refuse_unimplemented_ownership_annotation(argument);
                }
            }
            Type::Function(parameters, result) => {
                for parameter in parameters {
                    self.refuse_unimplemented_ownership_annotation(parameter);
                }
                self.refuse_unimplemented_ownership_annotation(result);
            }
            Type::Ref(inner) | Type::RefMut(inner) | Type::MutParam(inner) => {
                self.refuse_unimplemented_ownership_annotation(inner);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Check source directly so ownership diagnostics cannot be supplied by a native compiler failure.
    fn checked(source: &str) -> Result<TypeChecker, Box<dyn std::error::Error>> {
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut checker = TypeChecker::new();
        if let Err(errors) = checker.check_program(&program) {
            checker.errors = errors;
        }
        Ok(checker)
    }

    /// A source declaration shadowing a prelude spelling remains an ordinary nominal type.
    #[test]
    fn shadowed_mode_is_not_an_ownership_contract() -> Result<(), Box<dyn std::error::Error>> {
        let checker = checked(
            "model Borrowed[T]:\n    value: T\ndef identity(value: Borrowed[int]) -> Borrowed[int]:\n    return value\n",
        )?;
        assert!(checker.errors.is_empty(), "{:?}", checker.errors);
        Ok(())
    }

    /// Every explicit form is refused before executable storage can be selected, also when nested.
    #[test]
    fn modes_are_refused_at_every_annotation_depth() -> Result<(), Box<dyn std::error::Error>> {
        for mode in ["Borrowed", "BorrowedMut", "Owned", "Reference"] {
            let checker = checked(&format!("def read(value: Option[{mode}[str]]) -> None:\n    pass\n"))?;
            assert!(
                checker
                    .errors
                    .iter()
                    .any(|error| error.stable_code() == Some("INCAN-T0123")),
                "{mode}: {:?}",
                checker.errors
            );
        }
        Ok(())
    }

    /// Even a single-source signature needs the explicit relationship, and globals cannot fulfill it.
    #[test]
    fn borrowed_result_source_is_mandatory_and_local_to_signature() -> Result<(), Box<dyn std::error::Error>> {
        for source in [
            "def view(value: Borrowed[str]) -> Borrowed[str]:\n    return value\n",
            "def view(value: Borrowed[str]) -> Result[Borrowed[str], str]:\n    return Ok(value)\n",
            "def view(value: Borrowed[str]) -> FrozenList[Borrowed[str]]:\n    return []\n",
            "const other: str = \"outside\"\ndef view(value: str) -> str from other:\n    return value\n",
        ] {
            let checker = checked(source)?;
            assert!(
                checker
                    .errors
                    .iter()
                    .any(|error| error.stable_code() == Some("INCAN-T0124")),
                "{:?}",
                checker.errors
            );
        }
        Ok(())
    }

    /// A valid written source is still refused rather than authorizing old copied-result behavior.
    #[test]
    fn source_relationship_does_not_enable_execution() -> Result<(), Box<dyn std::error::Error>> {
        let checker = checked("def view(value: str) -> str from value:\n    return value\n")?;
        assert!(
            checker
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0123")),
            "{:?}",
            checker.errors
        );
        Ok(())
    }

    /// The known local-copy migration warns without rejecting or changing the program.
    #[test]
    fn mutable_stored_string_copy_warns() -> Result<(), Box<dyn std::error::Error>> {
        let checker = checked(
            "model Page:\n    title: str\ndef main() -> None:\n    page = Page(title=\"old\")\n    mut title = page.title\n    title = \"new\"\n",
        )?;
        assert!(checker.errors.is_empty(), "{:?}", checker.errors);
        assert!(
            checker
                .warnings
                .iter()
                .any(|warning| warning.message.contains("copies a place that a place binding borrows")),
            "{:?}",
            checker.warnings
        );
        Ok(())
    }
    /// Compatible annotations, scalar fields and read-only locals do not accidentally request independent copies.
    #[test]
    fn place_migration_includes_annotations_scalars_and_readonly() -> Result<(), Box<dyn std::error::Error>> {
        let checker = checked(
            "model Page:\n    count: int\ndef main() -> None:\n    page = Page(count=1)\n    mut count: int = page.count\n    let saved = page.count\n    copy = int(2)\n    fresh = Page(count=2)\n",
        )?;
        assert!(checker.errors.is_empty(), "{:?}", checker.errors);
        let warnings: Vec<_> = checker
            .warnings
            .iter()
            .filter(|warning| warning.stable_code() == Some("INCAN-T0125"))
            .collect();
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        Ok(())
    }

    /// Abstract and concrete receiver sources share checked slots; concrete field paths retain their declaration.
    #[test]
    fn receiver_contracts_cover_abstract_and_concrete_methods() -> Result<(), Box<dyn std::error::Error>> {
        for (source, fields) in [
            ("pub trait Reader:\n    def view(self) -> Borrowed[str] from self\n", 0),
            (
                "pub class Page:\n    title: str\n    def view(self) -> Borrowed[str] from self.title:\n        return self.title\n",
                1,
            ),
        ] {
            let checker = checked(source)?;
            let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
            let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
            let exports = crate::library_exports::collect_checked_public_exports(&program, &checker);
            let methods = match &exports.first().ok_or("missing public owner")?.kind {
                crate::library_exports::CheckedExportKind::Class(owner) => &owner.methods,
                crate::library_exports::CheckedExportKind::Trait(owner) => &owner.methods,
                _ => return Err("wrong owner kind".into()),
            };
            assert!(methods.first().ok_or("missing public method")?.ownership.is_some());
            let contract = checker
                .type_info
                .declarations
                .ownership_contracts
                .values()
                .next()
                .ok_or("missing method contract")?;
            let dependency = contract.result_source.as_ref().ok_or("missing receiver source")?;
            assert_eq!(dependency.slot, crate::ownership_metadata::SourceSlot::Receiver);
            assert_eq!(dependency.fields.len(), fields);
            assert!(
                checker
                    .errors
                    .iter()
                    .any(|error| error.stable_code() == Some("INCAN-T0123"))
            );
        }
        Ok(())
    }

    /// Annotation constructors and aliases cannot silently opt into ordinary generic value behavior.
    #[test]
    fn modes_are_not_wrappers_or_alias_escape_hatches() -> Result<(), Box<dyn std::error::Error>> {
        for source in [
            "def main() -> None:\n    value = Owned(1)\n",
            "type View = Borrowed[str]\ndef view(value: View) -> None:\n    pass\n",
            "def read(value: Owned[int, str]) -> None:\n    pass\n",
        ] {
            let checker = checked(source)?;
            assert!(
                checker
                    .errors
                    .iter()
                    .any(|error| error.stable_code() == Some("INCAN-T0123")),
                "{:?}",
                checker.errors
            );
        }
        Ok(())
    }
    /// Source relationships use slots and canonical fields, including substituted nested generic owners.
    #[test]
    fn checked_source_retains_nested_field_identities() -> Result<(), Box<dyn std::error::Error>> {
        let checker = checked(
            "model Address:\n    street: str\nmodel Customer[T]:\n    address: T\ndef view(customer: Borrowed[Customer[Address]]) -> Borrowed[str] from customer.address.street:\n    return customer.address.street\n",
        )?;
        let contract = checker
            .type_info
            .declarations
            .ownership_contracts
            .values()
            .next()
            .ok_or("missing ownership contract")?;
        let source = contract.result_source.as_ref().ok_or("missing checked source")?;
        assert_eq!(source.slot, crate::ownership_metadata::SourceSlot::Parameter(0));
        assert_eq!(
            source
                .fields
                .iter()
                .map(|field| field.declaration_name.as_str())
                .collect::<Vec<_>>(),
            ["address", "street"]
        );
        assert!(
            source
                .fields
                .iter()
                .all(|field| field.kind == incan_semantics_core::SemanticSourceTargetKind::Field)
        );
        assert_eq!(contract.modes.len(), 2);
        Ok(())
    }

    /// An alias-expanded borrowed result still requires a source, and impossible fields have source diagnostics.
    #[test]
    fn checked_alias_and_invalid_field_contracts() -> Result<(), Box<dyn std::error::Error>> {
        let missing =
            checked("type View[T] = Borrowed[T]\ndef view(value: View[str]) -> View[str]:\n    return value\n")?;
        assert!(
            missing
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0124"))
        );
        let invalid = checked(
            "model Page:\n    title: str\ndef view(page: Borrowed[Page]) -> Borrowed[str] from page.absent:\n    return \"\"\n",
        )?;
        assert!(
            invalid
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0124"))
        );
        assert!(invalid.type_info.declarations.ownership_contracts.is_empty());
        Ok(())
    }
}

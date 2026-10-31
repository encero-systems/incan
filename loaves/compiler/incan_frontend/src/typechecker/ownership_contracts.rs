//! Recognition of RFC 164 contracts before their executable ownership stages are enabled.
//!
//! Source parsing and formatting preserve declarations. Every resolved prelude mode and result-source clause is
//! refused before native lowering until checked loan and transfer facts can implement it without repair copies.

use incan_lang::lang::types::ownership::{self, OwnershipMode};
use incan_semantics_core::SymbolOrigin;

use super::TypeChecker;
use crate::ast::{AssignmentStmt, BindingKind, Expr, Param, ResultSource, Span, Spanned, Type};
use crate::diagnostics::CompileError;
use crate::symbols::{ResolvedType, TypeInfo};

impl TypeChecker {
    /// Warn about the measured mutable string-field copy before adopting RFC 129 write-through locals.
    ///
    /// This deliberately covers only proven stored fields. Calls, computed properties, explicit annotations and
    /// clones do not enter this migration cohort, and this warning does not change their runtime semantics.
    pub(super) fn warn_field_copy_migration(&mut self, assignment: &AssignmentStmt, ty: &ResolvedType) {
        if assignment.binding != BindingKind::Mutable || assignment.ty.is_some() || ty != &ResolvedType::Str {
            return;
        }
        let Expr::Field(base, field) = &assignment.value.node else {
            return;
        };
        let Some(ResolvedType::Named(owner)) = self.type_info.expr_type(base.span) else {
            return;
        };
        let stored = match self.lookup_type_info(owner) {
            Some(TypeInfo::Model(model)) => model.fields.contains_key(field),
            Some(TypeInfo::Class(class)) => class.fields.contains_key(field),
            _ => false,
        };
        if stored {
            self.warnings.push(CompileError::warning(
            format!("Mutable local '{}' currently copies this field; a future ownership release will make this a write-through binding", assignment.name),
            assignment.value.span,
        ).with_note("Runtime behavior is unchanged in this release")
            .with_hint("Use an explicit clone when an independent mutable snapshot is intended"));
        }
    }

    /// Resolve a mode only when ordinary lookup selects its canonical builtin declaration.
    fn explicit_ownership_mode(&self, name: &str) -> Option<OwnershipMode> {
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
                            "Explicit ownership contract '{}[T]' is recognized but not executable yet",
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

    /// Find explicit borrowed components in a written result, including outcome and tuple payloads.
    fn annotation_contains_borrowed_contract(&self, annotation: &Type) -> bool {
        match annotation {
            Type::Generic(name, arguments) => {
                matches!(
                    self.explicit_ownership_mode(name),
                    Some(OwnershipMode::Borrowed | OwnershipMode::BorrowedMut | OwnershipMode::Reference)
                ) || arguments
                    .iter()
                    .any(|argument| self.annotation_contains_borrowed_contract(&argument.node))
            }
            Type::DottedGeneric(_, arguments) | Type::Tuple(arguments) => arguments
                .iter()
                .any(|argument| self.annotation_contains_borrowed_contract(&argument.node)),
            Type::Ref(inner) | Type::RefMut(inner) | Type::MutParam(inner) => {
                self.annotation_contains_borrowed_contract(&inner.node)
            }
            _ => false,
        }
    }

    /// Validate a written source root against the declaration's own parameters or receiver, then refuse execution.
    ///
    /// A global or local with the same name never satisfies this declaration contract. Static field resolution and
    /// proof that returned values originate there remain an enabling-stage requirement, not a spelling inference.
    pub(super) fn check_result_source_contract(
        &mut self,
        source: Option<&Spanned<ResultSource>>,
        return_type: &Spanned<Type>,
        parameters: &[Spanned<Param>],
        receiver: Option<&str>,
    ) {
        let borrowed = self.annotation_contains_borrowed_contract(&return_type.node);
        let Some(source) = source else {
            if borrowed {
                self.errors.push(
                    CompileError::type_error(
                        "An explicit borrowed result requires 'from source', even with only one parameter".to_string(),
                        return_type.span,
                    )
                    .with_stable_code("INCAN-T0124")
                    .with_hint("Name the parameter or receiver and any static field path after the return type"),
                );
            }
            return;
        };
        let name = source.node.binding.node.as_str();
        if !parameters.iter().any(|parameter| parameter.node.name == name) && receiver != Some(name) {
            self.errors.push(
                CompileError::type_error(
                    format!("Result source '{name}' must name this declaration's parameter or receiver"),
                    source.node.binding.span,
                )
                .with_stable_code("INCAN-T0124"),
            );
            return;
        }
        self.errors.push(
            CompileError::type_error(
                "Borrowed result-source contracts are recognized but not executable yet".to_string(),
                source.span,
            )
            .with_stable_code("INCAN-T0123")
            .with_note("The compiler must prove the source relationship before accepting this declaration"),
        );
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
                .any(|warning| warning.message.contains("future ownership release")),
            "{:?}",
            checker.warnings
        );
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
}

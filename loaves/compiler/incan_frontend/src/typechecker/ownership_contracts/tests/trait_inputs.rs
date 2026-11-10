//! Trait-owned inputs retain their permissions across implementation spelling and module boundaries.

use super::{OwnershipMode, ResolvedType, TypeChecker, checked};

/// Inherited shared access forbids mutation and transfer even when an implementation writes an ordinary parameter.
#[test]
fn inherited_shared_inputs_enforce_body_permissions() -> Result<(), Box<dyn std::error::Error>> {
    for body in ["value.append(7)", "consume(value)"] {
        let source = format!(
            "def consume(value: Owned[list[int]]) -> None:\n    pass\ntrait Observer:\n    def observe(self, value: Borrowed[list[int]]) -> None:\n        pass\nclass Reader with Observer:\n    def observe(self, mut value: list[int]) -> None:\n        {body}\n"
        );
        let checker = checked(&source)?;
        assert!(
            checker
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0126")),
            "{source}: {:?}",
            checker.errors
        );
    }
    Ok(())
}

/// Explicit contrary spellings and competing trait contracts cannot quietly select one ABI by adoption order.
#[test]
fn trait_input_conflicts_are_source_diagnostics() -> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "trait Observer:\n    def observe[T](self, value: Borrowed[T]) -> None:\n        pass\nclass Reader with Observer:\n    def observe[T](self, value: Owned[T]) -> None:\n        pass\n",
        "trait Reader:\n    def use[T](self, value: Borrowed[T]) -> None:\n        pass\ntrait Taker:\n    def use[T](self, value: Owned[T]) -> None:\n        pass\nclass Both with Reader, Taker:\n    def use[T](self, value: T) -> None:\n        pass\n",
    ] {
        let checker = checked(source)?;
        assert!(
            checker
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0123") && error.message.contains("ownership")),
            "{source}: {:?}",
            checker.errors
        );
    }
    Ok(())
}

/// An inherited generic supertrait contract and a distinct implementation binder close to the implementation's type.
#[test]
fn trait_inputs_close_generics_and_supertraits() -> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "trait Observer[A]:\n    def observe(self, value: Borrowed[A]) -> None:\n        pass\ntrait TextObserver with Observer[str]:\n    def ready(self) -> bool:\n        return true\nclass Reader with TextObserver:\n    def observe(self, value: str) -> None:\n        pass\ndef run() -> None:\n    reader = Reader()\n    text = \"hello\"\n    reader.observe(text)\n    assert len(text) == 5\n",
        "trait Observer:\n    def observe[T](self, value: Borrowed[T]) -> None:\n        pass\nclass Reader with Observer:\n    def observe[U](self, value: U) -> None:\n        pass\ndef run() -> None:\n    reader = Reader()\n    reader.observe(\"hello\")\n",
    ] {
        let checker = checked(source)?;
        assert!(checker.errors.is_empty(), "{source}: {:?}", checker.errors);
        let contract = checker
            .type_info
            .declarations
            .ownership_contracts
            .iter()
            .filter(|(identity, _)| identity.declaration_name == "observe")
            .max_by_key(|(identity, _)| identity.declaration_span.start)
            .map(|(_, contract)| contract)
            .ok_or("missing implementation contract")?;
        assert_eq!(contract.modes[0].mode, OwnershipMode::Borrowed);
    }
    Ok(())
}

/// Imported trait facts remain authoritative when a consumer supplies a plain implementation.
#[test]
fn imported_trait_inputs_are_inherited() -> Result<(), Box<dyn std::error::Error>> {
    let dependency = "pub trait Observes:\n    def observe[T](self, value: Borrowed[T]) -> None:\n        pass\n";
    let source = "from protocols import Observes\nclass Observer with Observes:\n    def observe[T](self, value: T) -> None:\n        pass\ndef run() -> None:\n    observer = Observer()\n    value = \"hello\"\n    observer.observe(value)\n    assert len(value) == 5\n";
    let dependency_tokens = crate::lexer::lex(dependency).map_err(|errors| format!("{errors:?}"))?;
    let dependency = crate::parser::parse(&dependency_tokens).map_err(|errors| format!("{errors:?}"))?;
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_with_imports(&program, &[("protocols", &dependency)])
        .map_err(|errors| format!("{errors:?}"))?;
    let crate::ast::Declaration::Class(owner) = &program.declarations[1].node else {
        return Err("missing Observer".into());
    };
    let binding = checker
        .type_info
        .declarations
        .method_bindings_by_span
        .get(&owner.methods[0].span)
        .ok_or("missing implementation binding")?;
    let identity = binding.identity.as_ref().ok_or("missing identity")?;
    let contract = checker
        .type_info
        .declarations
        .ownership_contracts
        .get(identity)
        .ok_or("missing imported contract")?;
    assert_eq!(contract.modes[0].mode, OwnershipMode::Borrowed);
    assert!(
        matches!(&binding.params[0].ty, ResolvedType::Generic(name, _) if name == "Borrowed"),
        "{binding:?}"
    );
    // Ordinary generic trait receivers require the imported declaration header too. Equal declaration offsets in
    // different modules must not collide, and a shared input never authorizes a mutable receiver.
    let first = "pub trait First:\n    def read(self) -> str: ...\n";
    let other = "pub trait Other:\n    def read(self) -> int: ...\n";
    let first = crate::parser::parse(&crate::lexer::lex(first).map_err(|errors| format!("{errors:?}"))?)
        .map_err(|errors| format!("{errors:?}"))?;
    let other = crate::parser::parse(&crate::lexer::lex(other).map_err(|errors| format!("{errors:?}"))?)
        .map_err(|errors| format!("{errors:?}"))?;
    let source = "from first import First\nfrom other import Other\ndef text[T with First](value: Borrowed[T]) -> str:\n    return value.read()\ndef number[T with Other](value: Borrowed[T]) -> int:\n    return value.read()\n";
    let program = crate::parser::parse(&crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?)
        .map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_with_imports(&program, &[("first", &first), ("other", &other)])
        .map_err(|errors| format!("{errors:?}"))?;
    assert_eq!(checker.type_info.declarations.imported_method_headers.len(), 2);
    assert_eq!(checker.type_info.protocols.explicit_input_calls.len(), 2);
    for call in checker.type_info.protocols.explicit_input_calls.values() {
        assert!(checker.type_info.selected_input_contract(call).is_ok());
        let owner = call.owner.as_ref().ok_or("missing caller identity")?;
        assert!(checker.type_info.checked_input_body(owner)?.is_some());
        let mut missing = checker.type_info.clone();
        missing.declarations.imported_method_headers.remove(&call.callee);
        assert!(missing.selected_input_contract(call).is_err());
        let mut wrong_receiver = call.clone();
        wrong_receiver.receiver = Some(crate::ast::Receiver::Mutable);
        assert!(checker.type_info.selected_input_contract(&wrong_receiver).is_err());
    }
    // The native driver installs sealed interfaces instead of recollecting these declarations.
    let mut interface_builder = TypeChecker::new();
    for name in ["first", "other"] {
        interface_builder.register_dependency_module_path_segments(name, vec![name.to_string()]);
    }
    let interfaces = interface_builder.build_dependency_interface_component(
        &[
            (
                "first",
                &first,
                std::sync::Arc::from(crate::module::exported_symbols(&first)),
            ),
            (
                "other",
                &other,
                std::sync::Arc::from(crate::module::exported_symbols(&other)),
            ),
        ],
        &[],
        true,
    );
    let mut shared = TypeChecker::new();
    for name in ["first", "other"] {
        shared.register_dependency_module_path_segments(name, vec![name.to_string()]);
    }
    shared.select_dependency_interfaces(interfaces);
    shared
        .check_with_imports(&program, &[("first", &first), ("other", &other)])
        .map_err(|errors| format!("{errors:?}"))?;
    assert_eq!(shared.type_info.declarations.imported_method_headers.len(), 2);
    assert_eq!(shared.type_info.protocols.explicit_input_calls.len(), 2);
    for call in shared.type_info.protocols.explicit_input_calls.values() {
        assert!(shared.type_info.selected_input_contract(call).is_ok());
        assert!(
            shared
                .type_info
                .checked_input_body(call.owner.as_ref().ok_or("missing shared caller")?)?
                .is_some()
        );
    }
    let mutable = "pub trait Editor:\n    def change(mut self) -> None: ...\n";
    let mutable = crate::parser::parse(&crate::lexer::lex(mutable).map_err(|errors| format!("{errors:?}"))?)
        .map_err(|errors| format!("{errors:?}"))?;
    let source =
        "from editor import Editor\ndef change[T with Editor](value: Borrowed[T]) -> None:\n    value.change()\n";
    let program = crate::parser::parse(&crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?)
        .map_err(|errors| format!("{errors:?}"))?;
    assert!(
        TypeChecker::new()
            .check_with_imports(&program, &[("editor", &mutable)])
            .is_err()
    );
    Ok(())
}

/// Refreshing a dependency must remove its old input contracts, not retain them by stale declaration spans.
#[test]
fn dependency_refresh_replaces_input_contracts() -> Result<(), Box<dyn std::error::Error>> {
    let mut checker = TypeChecker::new();
    for (source, expected) in [
        (
            "pub trait Observes:\n    def observe(self, value: Borrowed[str]) -> None:\n        pass\n",
            1,
        ),
        (
            "pub trait Observes:\n    def observe(self, value: str) -> None:\n        pass\n",
            0,
        ),
    ] {
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        checker.import_module(&program, "protocols");
        assert_eq!(checker.dependency_method_input_contracts.len(), expected);
        assert_eq!(checker.dependency_method_headers.len(), 1);
        let header = checker
            .dependency_method_headers
            .values()
            .next()
            .ok_or("missing imported header")?;
        assert_eq!(
            matches!(&header.params[0].ty, ResolvedType::Generic(name, _) if name == "Borrowed"),
            expected == 1,
        );
    }
    Ok(())
}

/// Every concrete owner and explicitly targeted slot retains its own inherited permission.
#[test]
fn trait_input_contracts_cover_nominal_owners_and_targets() -> Result<(), Box<dyn std::error::Error>> {
    for declaration in [
        "model Reader with Observer:\n    marker: int\n    def observe[T](self, value: T) -> None:\n        pass\n",
        "enum Reader with Observer:\n    Ready\n    def observe[T](self, value: T) -> None:\n        pass\n",
        "type Reader = newtype int with Observer:\n    def observe[T](self, value: T) -> None:\n        pass\n",
        "class Reader with Observer, Taker:\n    def observe[T](self, value: T) for Observer -> None:\n        pass\n    def observe[T](self, value: T) for Taker -> None:\n        pass\n",
    ] {
        let source = format!(
            "trait Observer:\n    def observe[T](self, value: Borrowed[T]) -> None:\n        pass\ntrait Taker:\n    def observe[T](self, value: Owned[T]) -> None:\n        pass\n{declaration}"
        );
        let checker = checked(&source)?;
        assert!(checker.errors.is_empty(), "{source}: {:?}", checker.errors);
        let modes: Vec<_> = checker
            .type_info
            .declarations
            .ownership_contracts
            .iter()
            .filter(|(identity, _)| identity.declaration_span.start >= source.len() - declaration.len())
            .flat_map(|(_, contract)| contract.modes.iter().map(|mode| mode.mode))
            .collect();
        assert!(modes.contains(&OwnershipMode::Borrowed), "{source}: {modes:?}");
        if declaration.contains("for Taker") {
            assert!(modes.contains(&OwnershipMode::Owned), "{source}: {modes:?}");
        }
    }
    Ok(())
}

/// Overloads implementing different instantiations inherit only their matching slot, not sibling permissions.
#[test]
fn distinct_trait_instantiations_keep_distinct_input_contracts() -> Result<(), Box<dyn std::error::Error>> {
    let source = "trait Observer[T]:\n    def observe(self, value: Borrowed[T]) -> None: ...\nclass Both with Observer[int], Observer[str]:\n    def observe(self, value: int) -> None:\n        pass\n    def observe(self, value: str) -> None:\n        pass\n";
    let checker = checked(source)?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    Ok(())
}

use super::*;

/// A match's payload ownership constrains its subject, not an unrelated model read inside the same arm. Moving a
/// payload out must keep the subject owned while the independently observed dictionary can still be borrowed.
#[test]
fn enum_match_keeps_unrelated_inputs_shared() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
model Dictionary:
    values: list[str]
enum Node:
    Scalar(str)
    Other

def inspect(node: Node, dictionary: Dictionary) -> int:
    match node:
        Node.Scalar(name) => return len(name) + len(dictionary.values)
        Node.Other => return len(dictionary.values)
    return 0

def take(node: Node, dictionary: Dictionary) -> str:
    match node:
        Node.Scalar(name) =>
            assert len(dictionary.values) >= 0
            return name
        Node.Other => return ""
    return ""
"#;
    let (_, _, mut module) = checked_seed(source)?;
    super::shared_reads::retain_modules(std::slice::from_mut(&mut module), &[]);
    for name in ["inspect", "take"] {
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == name)
            .ok_or("missing enum match body")?;
        assert_eq!(
            body.params[1].input_access,
            incan_semantics_core::IncanInputAccess::InferredShared,
            "{name}: {body:#?}"
        );
        if name == "take" {
            assert_eq!(
                body.params[0].input_access,
                incan_semantics_core::IncanInputAccess::Inferred,
                "moving a payload must retain the owned subject: {body:#?}"
            );
        }
    }
    Ok(())
}

/// An immediate error callback borrows the input it only observes, without turning a stored closure into an alias.
#[test]
fn immediate_error_callbacks_borrow_read_only_captures() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
class Cursor:
    items: list[int]
    index: int = 0
    pub def peek(self) -> int:
        return self.items[self.index]
    pub def take(mut self) -> int:
        value = self.items[self.index]
        self.index += 1
        return value
model Shape:
    identity: str
    metadata: list[str]
model Field:
    shape: Shape

def parser(mut cursor: Cursor, outcome: Result[int, str]) -> Result[int, str]:
    outcome.map_err((_) => f"at {cursor.peek()}")?
    return Ok(cursor.take())

def read_field(field: Field, outcome: Result[int, str]) -> Result[int, str]:
    return outcome.map_err((error) => f"{field.shape.identity}: {error}")

def retained(field: Field) -> int:
    callback = () => len(field.shape.metadata)
    return callback()

def take_field(field: Field) -> Field:
    return field

def through_owned(field: Field) -> str:
    return take_field(field).shape.identity

def dependent(field: Field, outcome: Result[int, str]) -> Result[int, str]:
    return outcome.map_err((_) => through_owned(field))

def returned(field: Field, outcome: Result[int, str]) -> Result[int, Field]:
    return outcome.map_err((_) => field)

def store_text(text: str) -> list[str]:
    return [text]

def text_message(error: str, text: str) -> str:
    stored = store_text(text)
    return error + str(len(stored))

def field_dependent(field: Field, outcome: Result[int, str]) -> Result[int, str]:
    return outcome.map_err((error) => text_message(error, field.shape.identity))

def invoke(callback: Callable[str, str]) -> str:
    return callback("invalid")

class Runner:
    pub def map_err(self, callback: Callable[str, str]) -> str:
        return callback("invalid")

def direct(field: Field) -> str:
    return invoke((error) => f"{field.shape.identity}: {error}")

def method(field: Field, runner: Runner) -> str:
    return runner.map_err((error) => f"{field.shape.identity}: {error}")

def advance(mut cursor: Cursor, callback: Callable[(), int]) -> int:
    cursor.take()
    return callback()

def overlap(mut cursor: Cursor) -> int:
    return advance(cursor, () => cursor.peek())

"#;
    let (_, _, mut module) = checked_seed(source)?;
    super::shared_reads::retain_modules(std::slice::from_mut(&mut module), &[]);
    for name in [
        "parser",
        "read_field",
        "direct",
        "method",
        "overlap",
        "retained",
        "dependent",
        "returned",
        "field_dependent",
    ] {
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == name)
            .ok_or("missing callback body")?;
        let captures = body
            .block
            .stmts
            .iter()
            .find_map(|statement| match &statement.kind {
                bir::StatementKind::Assign {
                    rvalue: bir::Rvalue::Closure { captured_operands, .. },
                    ..
                } => Some(captured_operands),
                _ => None,
            })
            .ok_or("missing closure capture")?;
        let Some(bir::Operand::Place(capture)) = captures.first() else {
            return Err("missing captured source place".into());
        };
        if matches!(name, "parser" | "read_field" | "direct" | "method") {
            assert_eq!(capture.fact, bir::OwnershipFact::Borrow, "{name}: {body:#?}");
            assert!(!capture.last_use);
        } else {
            assert_ne!(
                capture.fact,
                bir::OwnershipFact::Borrow,
                "stored or consuming closure changed: {body:#?}"
            );
        }
        if matches!(name, "read_field" | "direct" | "method") {
            assert_eq!(
                body.params[0].input_access,
                incan_semantics_core::IncanInputAccess::InferredShared,
                "{body:#?}"
            );
        } else if name != "parser" {
            assert_eq!(
                body.params[0].input_access,
                incan_semantics_core::IncanInputAccess::Inferred,
                "{name}: {body:#?}"
            );
        }
    }
    Ok(())
}

/// Copy projections remain reads inside value wrappers; a wrapper containing an owner still needs that owner.
#[test]
fn wrapped_scalar_projections_keep_inputs_shared() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
model Packet:
    value: int
    slots: dict[str, int]
    metadata: list[str]

def ok_field(packet: Packet) -> Result[int, str]:
    return Ok(packet.value)
def err_field(packet: Packet) -> Result[int, int]:
    return Err(packet.value)
def some_field(packet: Packet) -> Option[int]:
    return Some(packet.value)
def ok_index(packet: Packet) -> Result[int, str]:
    return Ok(packet.slots["key"])
def plain_field(packet: Packet) -> int:
    return packet.value

def whole_ok(packet: Packet) -> Result[Packet, str]:
    return Ok(packet)
def whole_some(packet: Packet) -> Option[Packet]:
    return Some(packet)
def owned_ok(packet: Packet) -> Result[list[str], str]:
    return Ok(packet.metadata)
def owned_err(packet: Packet) -> Result[int, list[str]]:
    return Err(packet.metadata)
def owned_some(packet: Packet) -> Option[list[str]]:
    return Some(packet.metadata)
"#;
    let (_, _, mut module) = checked_seed(source)?;
    super::shared_reads::retain_modules(std::slice::from_mut(&mut module), &[]);
    for (name, expected) in [
        ("ok_field", incan_semantics_core::IncanInputAccess::InferredShared),
        ("err_field", incan_semantics_core::IncanInputAccess::InferredShared),
        ("some_field", incan_semantics_core::IncanInputAccess::InferredShared),
        ("ok_index", incan_semantics_core::IncanInputAccess::InferredShared),
        ("plain_field", incan_semantics_core::IncanInputAccess::InferredShared),
        ("whole_ok", incan_semantics_core::IncanInputAccess::Inferred),
        ("whole_some", incan_semantics_core::IncanInputAccess::Inferred),
        ("owned_ok", incan_semantics_core::IncanInputAccess::Inferred),
        ("owned_err", incan_semantics_core::IncanInputAccess::Inferred),
        ("owned_some", incan_semantics_core::IncanInputAccess::Inferred),
    ] {
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == name)
            .ok_or("missing wrapper body")?;
        assert_eq!(body.params[0].input_access, expected, "{name}: {body:#?}");
    }
    Ok(())
}

/// Producer namespaces survive checker reset and lowering without changing canonical compiler identities.
pub(super) fn assert_declared_record_namespace() -> Result<(), Box<dyn std::error::Error>> {
    let source = "pub model Value:\n    pub number: int\npub enum Choice:\n    Number(int)\n    Empty\npub class Resource:\n    pub number: int\n";
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let declared = incan_semantics_core::record_identity::RecordPackageIdentity {
        registry_id: "sha256:3d7781b11f6b80f950bab17238cffa7c1f77b5e1f0e0dbbaa656149d9ffbffb0".to_owned(),
        scoped_name: "example/catalog".to_owned(),
    };
    let mut original_identities = None;
    for package in [None, Some(declared)] {
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(vec!["records".to_owned()]));
        checker.set_current_package_identity(Some("example/catalog".to_owned()));
        checker.set_record_package_identity(package.clone());
        checker
            .check_program(&program)
            .map_err(|errors| format!("{errors:?}"))?;
        let facts = checker.into_type_info();
        if let Some(original) = &original_identities {
            assert_eq!(&facts.declarations.declaration_identities, original);
        } else {
            original_identities = Some(facts.declarations.declaration_identities.clone());
        }
        let providers = ProviderOperationCatalog::new();
        let context = BodyIrContext {
            modules: &[],
            provider_operations: &providers,
            registry_package: None,
            library_package: Some("example/catalog"),
            include_inline_tests: false,
        };
        let seed = super::build_module_seed(&program, &["records".to_owned()], &facts, &context);
        let model = seed
            .nominal_declarations
            .iter()
            .find(|owner| owner.canonical.declaration_name == "Value")
            .ok_or("missing model")?;
        assert_eq!(model.record_package_identity, package);
        let class = seed
            .nominal_declarations
            .iter()
            .find(|owner| owner.canonical.declaration_name == "Resource")
            .ok_or("missing class")?;
        assert!(class.record_package_identity.is_none());
        let choice = seed
            .enum_declarations
            .iter()
            .find(|owner| owner.canonical.declaration_name == "Choice")
            .ok_or("missing enum")?;
        assert_eq!(choice.record_package_identity, package);
    }
    Ok(())
}

/// A plain implementation inherits the explicit trait input contract before its body and callers are checked.
#[test]
fn trait_implementation_inherits_explicit_input_contract() -> Result<(), Box<dyn std::error::Error>> {
    for (mode, access) in [
        ("Borrowed", incan_semantics_core::IncanInputAccess::Shared),
        ("BorrowedMut", incan_semantics_core::IncanInputAccess::Exclusive),
        ("Owned", incan_semantics_core::IncanInputAccess::Owned),
    ] {
        let source = format!(
            "trait Observe:\n    def observe[T](self, value: {mode}[T]) -> None:\n        pass\nclass Observer with Observe:\n    def observe[T](self, value: T) -> None:\n        pass\n"
        );
        let (program, facts, seed) = checked_seed(&source)?;
        let crate::ast::Declaration::Class(owner) = &program.declarations[1].node else {
            return Err("missing Observer class".into());
        };
        let span = owner.methods[0].span;
        let body = seed
            .bodies
            .iter()
            .find(|body| {
                body.canonical
                    .as_ref()
                    .is_some_and(|identity| identity.declaration_span == span)
            })
            .ok_or("missing concrete observe body")?;
        assert_eq!(body.params[1].input_access, access, "{mode}: {body:#?}");
        assert!(
            matches!(body.params[1].ty, incan_semantics_core::IncanType::TypeVar(_)),
            "body type {:?}; binding {:?}; contracts {:?}",
            body.params[1].ty,
            facts.declarations.method_bindings_by_span.get(&span),
            facts.declarations.ownership_contracts
        );
    }
    Ok(())
}

/// An abstract trait declares the input ABI but does not acquire a fabricated executable body.
#[test]
fn abstract_trait_inputs_govern_the_concrete_body() -> Result<(), Box<dyn std::error::Error>> {
    let source = "trait Observe:\n    def observe[T](self, value: Borrowed[T]) -> None: ...\nclass Observer with Observe:\n    def observe[T](self, value: T) -> None:\n        pass\n";
    let (_, _, seed) = checked_seed(source)?;
    assert_eq!(
        seed.bodies.len(),
        1,
        "{:?}",
        seed.bodies.iter().map(|body| &body.name).collect::<Vec<_>>()
    );
    assert_eq!(
        seed.bodies[0].params[1].input_access,
        incan_semantics_core::IncanInputAccess::Shared
    );
    assert!(matches!(
        seed.bodies[0].params[1].ty,
        incan_semantics_core::IncanType::TypeVar(_)
    ));
    Ok(())
}

/// The source checker's conditional result survives executable serialization with declaration-scoped binders;
/// stripping the requirements cannot turn its generic profile into an unconditional source proof.
#[test]
fn generic_result_profile_retains_symbolic_requirement() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"trait Visit:
    def visit[E, W with Sink[E]](self, mut sink: BorrowedMut[W]) -> Result[None, E]:
        pass

trait Sink[E]:
    def feed[V with Visit](mut self, value: Borrowed[V]) -> Result[None, E]:
        return value.visit(self)

class Value with Visit:
    pub count: int
    def visit[E, W with Sink[E]](self, mut sink: W) -> Result[None, E]:
        return Ok(None)

class Reader with Sink[str]:
    pub count: int

def main() -> Result[None, str]:
    value = Value(count=7)
    mut reader = Reader(count=0)
    reader.feed(value)?
    assert value.count == 7
    return Ok(None)
"#;
    let (_, _, seed) = checked_seed(source)?;
    let body = seed
        .bodies
        .iter()
        .find(|body| {
            body.canonical
                .as_ref()
                .is_some_and(|owner| owner.declaration_name == "feed")
        })
        .ok_or("missing generic feed body")?;
    let profile = body
        .checked_input_profile
        .as_ref()
        .ok_or("missing generic input profile")?;
    assert_eq!(profile.version, 3);
    assert_eq!(profile.result_requirements.len(), 1);
    assert_eq!(
        profile.result_requirements[0].1,
        super::IncanType::Generic {
            base: "Result".into(),
            args: vec![
                super::IncanType::Primitive(super::IncanPrimitiveType::Unit),
                super::IncanType::TypeVar("E".into())
            ],
        }
    );
    let decoded: bir::Body = postcard::from_bytes(&postcard::to_allocvec(body)?)?;
    assert_eq!(decoded.checked_input_profile, body.checked_input_profile);
    let mut stripped = decoded;
    stripped
        .checked_input_profile
        .as_mut()
        .ok_or("lost profile")?
        .result_requirements
        .clear();
    assert!(stripped.input_profile_gap().is_some());
    Ok(())
}

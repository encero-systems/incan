//! RFC 164: a native `&T` reads as `Borrowed[T]`, and a `Borrowed[T]` value is used as a read-only `T` for every `T`.
//! Reads go through the borrow with no copy; an owned use gets an implicit clone, or is refused there when `T` has no
//! clone; an `Owned[T]` parameter and a `mut` binding need an explicit `.clone()`.

use super::*;
use crate::diagnostics::CompileError;
use crate::loaf_index::LoafIndex;
use incan_lang::interop::{
    RustFieldInfo, RustFunctionSig, RustItemKind, RustItemMetadata, RustTypeInfo, RustTypeShape, RustVisibility,
};
use std::collections::BTreeMap;
use std::sync::Arc;

/// A Rust function without parameters that returns `display`.
fn function(path: &str, display: &str) -> (String, RustItemMetadata) {
    (
        path.to_owned(),
        RustItemMetadata {
            canonical_path: path.into(),
            definition_path: None,
            visibility: RustVisibility::Public,
            kind: RustItemKind::Function(RustFunctionSig {
                receiver_contract: None,
                type_params: Vec::new(),
                bounds: Vec::new(),
                params: Vec::new(),
                return_type: display.into(),
                is_async: false,
                is_unsafe: false,
            }),
        },
    )
}

/// `demo::label() -> &String`, `demo::token() -> &Token` (no `Clone`) and the `Token` struct.
fn borrowed_facts() -> BTreeMap<String, RustItemMetadata> {
    let token = RustItemMetadata {
        canonical_path: "demo::Token".into(),
        definition_path: None,
        visibility: RustVisibility::Public,
        kind: RustItemKind::Type(RustTypeInfo {
            fields: vec![RustFieldInfo {
                name: "id".into(),
                type_display: "i64".into(),
                type_shape: RustTypeShape::Int,
            }],
            ..RustTypeInfo::default()
        }),
    };
    BTreeMap::from([
        function("demo::label", "&'static alloc::string::String"),
        function("demo::token", "&'static demo::Token"),
        ("demo::Token".to_owned(), token),
    ])
}

/// Check `source` against the borrowed facts. A refused Rust reference spelling is set aside: the twins below compare
/// what the rest of the check and Body IR make of it with the Incan spelling.
fn check(source: &str) -> Result<(ast::Program, TypeCheckInfo), Vec<CompileError>> {
    let mut loaves = LoafIndex::default();
    loaves.add_rust_facts(&Default::default(), borrowed_facts());
    let mut checker = TypeChecker::new();
    checker.set_loaves(Arc::new(loaves));
    let tokens = crate::lexer::lex(source).map_err(|_| Vec::new())?;
    let program = crate::parser::parse(&tokens).map_err(|_| Vec::new())?;
    if let Err(errors) = checker.check_program(&program) {
        let errors: Vec<CompileError> = errors
            .into_iter()
            .filter(|error| !error.message.contains("in Incan, not `&"))
            .collect();
        if !errors.is_empty() {
            return Err(errors);
        }
    }
    Ok((program, checker.into_type_info()))
}

/// One rendered body and its locals by name and identifier.
type RenderedBody = (String, Vec<(Option<String>, String)>);

/// Each body of `source`'s module, rendered, keyed by its function name.
fn rendered_bodies(source: &str) -> Result<BTreeMap<String, RenderedBody>, Box<dyn std::error::Error>> {
    let (program, facts) = check(source).map_err(|errors| format!("{errors:?}"))?;
    let providers = ProviderOperationCatalog::new();
    let context = BodyIrContext {
        modules: crate::body_ir::ContextModules::default(),
        provider_operations: &providers,
        registry_package: None,
        library_package: None,
        include_inline_tests: false,
    };
    let module = super::build_module_seed(&program, &[], &facts, &context);
    Ok(module
        .bodies
        .iter()
        .map(|body| {
            let locals = body
                .locals
                .iter()
                .map(|local| (local.name.clone(), format!("{:?}", local.id)))
                .collect();
            (body.name.clone(), (format!("{body:?}"), locals))
        })
        .collect())
}

/// Every message of a refused check.
fn refusals(source: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    match check(source) {
        Ok(_) => Err("the check was accepted".into()),
        Err(errors) => Ok(errors.iter().map(|error| format!("{error:?}")).collect()),
    }
}

/// Whether `body` reads the local named `name` with a clone anywhere.
fn clones_local(body: &str, locals: &[(Option<String>, String)], name: &str) -> bool {
    locals
        .iter()
        .filter(|(local, _)| local.as_deref() == Some(name))
        .any(|(_, id)| body.contains(&format!("root: Local({id}), projection: [] }}, fact: Clone")))
}

/// A borrowed text compares, tests membership, measures, formats, reads through methods and iterates with no copy of
/// the borrowed value.
#[test]
fn borrowed_text_reads_through_the_borrow() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from rust::demo import label\n\ndef reads(published: set[str]) -> int:\n    key = label()\n    mut found = 0\n    if key not in published:\n        found += 1\n    if key == \"x\":\n        found += 1\n    found += len(key)\n    note = f\"{key}!\"\n    if note.startswith(key):\n        found += 1\n    for letter in key:\n        if letter == \"a\":\n            found += 1\n    return found\n";
    let bodies = rendered_bodies(source)?;
    let (reads, locals) = bodies.get("reads").ok_or("missing body")?;
    assert!(!reads.contains("unsupported"), "{reads}");
    assert!(!clones_local(reads, locals, "key"), "{reads}");
    Ok(())
}

/// A borrowed Rust value reads its fields through the borrow.
#[test]
fn borrowed_rust_value_reads_its_fields() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from rust::demo import token\n\ndef read() -> int:\n    held = token()\n    return held.id + 1\n";
    let bodies = rendered_bodies(source)?;
    let (read, _) = bodies.get("read").ok_or("missing body")?;
    assert!(!read.contains("unsupported"), "{read}");
    assert!(!read.contains("fact: Clone"), "{read}");
    Ok(())
}

/// Storing a borrowed text, returning it as `str` and passing it to a `str` parameter taken by value each clone it.
#[test]
fn owned_uses_of_borrowed_text_clone_it() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from rust::demo import label\n\ndef keep(text: str) -> list[str]:\n    return [text]\n\ndef store() -> list[str]:\n    mut out: list[str] = []\n    out.append(label())\n    return out\n\ndef give() -> str:\n    return label()\n\ndef hand_on() -> list[str]:\n    return keep(label())\n";
    let bodies = rendered_bodies(source)?;
    for name in ["store", "give", "hand_on"] {
        let (body, _) = bodies.get(name).ok_or("missing body")?;
        assert!(!body.contains("unsupported"), "{name}: {body}");
        assert!(body.contains("fact: Clone"), "{name}: {body}");
    }
    Ok(())
}

/// An owned use of a borrowed value whose type has no clone is refused at that use.
#[test]
fn owned_use_without_clone_is_refused_there() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from rust::demo import Token, token\n\ndef give() -> Token:\n    return token()\n";
    let messages = refusals(source)?;
    assert!(messages.iter().any(|message| message.contains("clone")), "{messages:?}");
    Ok(())
}

/// A `mut` binding never clones a borrowed value implicitly, annotated or rebound; the diagnostic names `.clone()`.
#[test]
fn mut_bindings_need_an_explicit_clone() -> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "from rust::demo import label\n\ndef run() -> str:\n    mut text: str = label()\n    text = text + \"!\"\n    return text\n",
        "from rust::demo import label\n\ndef run() -> str:\n    mut text = label()\n    text = text + \"!\"\n    return text\n",
    ] {
        let messages = refusals(source)?;
        assert!(
            messages.iter().any(|message| message.contains(".clone()")),
            "{source}: {messages:?}"
        );
    }
    Ok(())
}

/// An `Owned[T]` parameter never clones a borrowed value implicitly; Body IR refuses the call and names `.clone()`.
#[test]
fn owned_parameters_need_an_explicit_clone() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from rust::demo import label\n\ndef own(text: Owned[str]) -> str:\n    return text\n\ndef run() -> str:\n    return own(label())\n";
    let bodies = rendered_bodies(source)?;
    let (run, _) = bodies.get("run").ok_or("missing body")?;
    assert!(run.contains("write `.clone()` on it"), "{run}");
    Ok(())
}

/// No diagnostic spells a borrowed type with `&`: a native `&T` is `Borrowed[T]` in every message.
#[test]
fn diagnostics_spell_native_references_as_borrowed() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from rust::demo import label\n\ndef run() -> int:\n    return label()\n";
    let messages = refusals(source)?;
    assert!(
        messages.iter().any(|message| message.contains("Borrowed[Str]")),
        "{messages:?}"
    );
    assert!(!messages.iter().any(|message| message.contains("&")), "{messages:?}");
    Ok(())
}

/// `demo::CopyNumber`: a Rust `Copy` struct whose `Clone` is observable, with a by-value `into_value(self)`.
fn copy_facts() -> BTreeMap<String, RustItemMetadata> {
    let implemented = |path: &str| incan_lang::interop::RustImplementedTrait {
        path: path.into(),
        mutable_reference: false,
        self_type: "demo::CopyNumber".into(),
        type_params: Vec::new(),
        type_arguments: Vec::new(),
        bounds: Vec::new(),
        associated_type_bindings: Vec::new(),
    };
    let into_value = incan_lang::interop::RustMethodSig {
        name: "into_value".into(),
        signature: RustFunctionSig {
            receiver_contract: None,
            type_params: Vec::new(),
            bounds: Vec::new(),
            params: vec![incan_lang::interop::RustParam {
                name: Some("self".into()),
                type_display: "Self".into(),
            }],
            return_type: "i64".into(),
            is_async: false,
            is_unsafe: false,
        },
        impl_type_params: Vec::new(),
        impl_self_type: Some("demo::CopyNumber".into()),
    };
    BTreeMap::from([(
        "demo::CopyNumber".to_owned(),
        RustItemMetadata {
            canonical_path: "demo::CopyNumber".into(),
            definition_path: None,
            visibility: RustVisibility::Public,
            kind: RustItemKind::Type(RustTypeInfo {
                fields: vec![RustFieldInfo {
                    name: "value".into(),
                    type_display: "i64".into(),
                    type_shape: RustTypeShape::Int,
                }],
                methods: vec![into_value],
                implemented_traits: vec![implemented("core::marker::Copy"), implemented("core::clone::Clone")],
                ..RustTypeInfo::default()
            }),
        },
    )])
}

/// A borrowed `Copy` value is copied, never cloned: through a by-value receiver, a by-value argument, a return, a
/// stored element and a field write. This is the `borrowed_rust_enum_payload` contract's `CopyNumber`, whose `Clone`
/// prints, behind the same `&CopyNumber` parameters.
#[test]
fn borrowed_copy_values_are_copied_not_cloned() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from rust::demo import CopyNumber\n\nmodel Holder:\n    number: CopyNumber\n\ndef keep(value: CopyNumber) -> list[CopyNumber]:\n    return [value]\n\ndef receiver(value: Borrowed[CopyNumber]) -> int:\n    return value.into_value()\n\ndef argument(value: Borrowed[CopyNumber]) -> list[CopyNumber]:\n    return keep(value)\n\ndef give(value: Borrowed[CopyNumber]) -> CopyNumber:\n    return value\n\ndef store(value: Borrowed[CopyNumber]) -> list[CopyNumber]:\n    mut out: list[CopyNumber] = []\n    out.append(value)\n    return out\n\ndef write(value: Borrowed[CopyNumber], mut holder: Holder) -> int:\n    holder.number = value\n    return holder.number.value\n";
    let mut loaves = LoafIndex::default();
    loaves.add_rust_facts(&Default::default(), copy_facts());
    let mut checker = TypeChecker::new();
    checker.set_loaves(Arc::new(loaves));
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let facts = checker.into_type_info();
    let providers = ProviderOperationCatalog::new();
    let context = BodyIrContext {
        modules: crate::body_ir::ContextModules::default(),
        provider_operations: &providers,
        registry_package: None,
        library_package: None,
        include_inline_tests: false,
    };
    let module = super::build_module_seed(&program, &[], &facts, &context);
    for name in ["receiver", "argument", "give", "store", "write"] {
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == name)
            .ok_or("missing body")?;
        let rendered = format!("{body:?}");
        assert!(!rendered.contains("unsupported"), "{name}: {rendered}");
        assert!(!rendered.contains("fact: Clone"), "{name}: {rendered}");
        assert!(!rendered.contains("\"clone\""), "{name}: {rendered}");
    }
    Ok(())
}

/// The native spellings of a shared and an exclusive borrow, padded to the width of the explicit ones so every span
/// of the two spellings agrees.
const NATIVE: (&str, &str) = ("&         {}", "&mut         {}");
/// The explicit spellings of a shared and an exclusive borrow.
const EXPLICIT: (&str, &str) = ("Borrowed[{}]", "BorrowedMut[{}]");

/// Expand `{borrow:T}` and `{borrow_mut:T}` markers with one pair of spellings; other braces pass through.
fn spelled(template: &str, (shared, exclusive): (&str, &str)) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let marker = &rest[start + 1..start + end];
        match marker.split_once(':') {
            Some(("borrow", name)) => out.push_str(&shared.replace("{}", name)),
            Some(("borrow_mut", name)) => out.push_str(&exclusive.replace("{}", name)),
            _ => out.push_str(&rest[start..=start + end]),
        }
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    out
}

/// RFC 164: `Borrowed[T]` and `&T`, and `BorrowedMut[T]` and `&mut T`, are one type. Every body of a module lowers to
/// Body IR: reads through the borrow, owned uses that clone, exclusive writes, calls that lend an owned local, local
/// borrows and the platform trace writer's `append(self, text)` and `write_record(self, row)` shapes. The Rust
/// spelling is refused before Body IR, so the bodies are those of the Incan spelling.
#[test]
fn explicit_borrows_lower_as_their_reference_twins() -> Result<(), Box<dyn std::error::Error>> {
    let template = "from rust::demo import label\n\nmodel Item:\n    name: str\n    count: int\n\nmodel Counter:\n    value: int\n\ndef echo(text: str) -> str:\n    return text\n\ndef keep(item: Item) -> list[Item]:\n    return [item]\n\ndef measure(text: {borrow:str}) -> int:\n    return len(echo(text))\n\ndef reads(item: {borrow:Item}, items: {borrow:list[Item]}) -> int:\n    mut total = item.count + len(items)\n    for entry in items:\n        total += entry.count\n    return total\n\ndef give(item: {borrow:Item}) -> Item:\n    return item\n\ndef hand_on(item: {borrow:Item}) -> list[Item]:\n    return keep(item)\n\ndef bump(mut counter: {borrow_mut:Counter}) -> int:\n    counter.value = counter.value + 1\n    return counter.value\n\ndef first(items: list[Item]) -> int:\n    head: {borrow:Item} = items[0]\n    return head.count\n\nclass Writer:\n    pub rows: list[str]\n\n    pub def append(mut self, text: {borrow:str}) -> int:\n        self.rows.append(echo(text))\n        return len(self.rows)\n\n    pub def write_record(mut self, row: {borrow:Item}) -> int:\n        self.rows.append(echo(row.name))\n        return row.count + len(self.rows)\n\ndef main() -> int:\n    mut counter = Counter(value=0)\n    item = Item(name=\"a\", count=1)\n    mut writer = Writer(rows=[])\n    return bump(counter) + measure(label()) + reads(item, [item]) + writer.append(\"x\") + writer.write_record(item)\n";
    let explicit = rendered_bodies(&spelled(template, EXPLICIT))?;
    for name in ["append", "write_record", "bump", "first", "main"] {
        assert!(explicit.contains_key(name), "{name}: {:?}", explicit.keys());
    }
    for (name, (body, _)) in &explicit {
        assert!(!body.contains("unsupported"), "{name}: {body}");
    }
    Ok(())
}

/// An owned use of a borrowed value whose type has no clone is refused at that use in both spellings, with the same
/// diagnostic; a borrowed `Copy` value is copied in both.
#[test]
fn explicit_borrows_refuse_and_copy_as_their_reference_twins() -> Result<(), Box<dyn std::error::Error>> {
    let template = "from rust::demo import Token\n\ndef give(value: {borrow:Token}) -> Token:\n    return value\n";
    let native = refusals(&spelled(template, NATIVE))?;
    let explicit = refusals(&spelled(template, EXPLICIT))?;
    assert_eq!(explicit, native);
    assert!(native.iter().any(|message| message.contains("clone")), "{native:?}");
    Ok(())
}

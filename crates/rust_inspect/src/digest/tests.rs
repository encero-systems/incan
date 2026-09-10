use std::collections::BTreeMap;
use std::error::Error;

use super::*;

/// Module path every fixture in this file is digested under.
const MODULE: &str = "probe";

/// The baseline fixture every invariant is measured against.
///
/// Its shape is deliberate rather than incidental. Each declaration an invariant edits has at least one further
/// declaration positioned *after* it — `Omega` and its `impl` come last and are never the edit target — because
/// positional leakage only contaminates what a parser reaches later. A fixture whose edit sits at the end cannot
/// detect a span reaching the digest, and would report a passing invariant it never exercised.
const BASE: &str = r#"#![allow(dead_code)]

pub struct Alpha {
    pub left: u8,
}

pub fn middle(value: u8) -> u8 {
    value + 1
}

pub struct Omega {
    pub right: u8,
}

impl Omega {
    pub fn tail(&self) -> u8 {
        self.right
    }
}
"#;

/// Digest a fixture into a rendered-key to digest map.
///
/// A repeated rendered key is reported as an error rather than silently overwritten, so every fixture in this file
/// doubles as a check that distinct declarations receive distinct keys.
fn digest_map(source: &str) -> Result<BTreeMap<String, String>, Box<dyn Error>> {
    let digest = digest_rust_source(MODULE, source)?;
    let mut map = BTreeMap::new();
    for item in digest.items() {
        if map.insert(item.key.render(), item.digest.clone()).is_some() {
            return Err(format!("two declarations share the key `{}`", item.key.render()).into());
        }
    }
    Ok(map)
}

/// Digest a fixture into a rendered-key to digest map, omitting declarations with the named identifiers.
///
/// Invariants that expect one declaration to move use this for everything else, so "other items unchanged" is
/// checked against complete keys and not only against digests.
fn digest_map_excluding(source: &str, excluded: &[&str]) -> Result<BTreeMap<String, String>, Box<dyn Error>> {
    let mut map = digest_map(source)?;
    map.retain(|key, _| !excluded.iter().any(|name| key.contains(&format!("|{name}|"))));
    Ok(map)
}

/// Return the single declaration matching an identity, failing when the fixture is ambiguous.
fn single<'a>(
    digest: &'a RustSourceDigest,
    owner: Option<&str>,
    kind: RustDigestItemKind,
    name: &str,
) -> Result<&'a RustItemDigest, Box<dyn Error>> {
    match digest.find(MODULE, owner, kind, name).as_slice() {
        [only] => Ok(only),
        found => Err(format!("expected exactly one `{name}` {kind:?}, found {}", found.len()).into()),
    }
}

/// Return the single declaration matching an identity in a freshly digested fixture.
fn single_of(
    source: &str,
    owner: Option<&str>,
    kind: RustDigestItemKind,
    name: &str,
) -> Result<RustItemDigest, Box<dyn Error>> {
    let digest = digest_rust_source(MODULE, source)?;
    Ok(single(&digest, owner, kind, name)?.clone())
}

/// Adding or removing `///` and `//!` documentation must move nothing.
///
/// Doc comments lex to `#[doc]` and `#![doc]` attributes, so they survive tokenization and have to be excluded
/// explicitly. The fixture documents an item, a struct field, a crate root, and an `impl` method, each with further
/// declarations after it.
#[test]
fn doc_comments_do_not_move_any_digest() -> Result<(), Box<dyn Error>> {
    let documented = r#"#![allow(dead_code)]
//! Crate prose that no compiled output depends on.

/// Alpha carries the left byte.
pub struct Alpha {
    /// The left byte itself.
    pub left: u8,
}

/// Middle adds one.
///
/// A second paragraph, to make the attribute count differ as well as its content.
pub fn middle(value: u8) -> u8 {
    value + 1
}

pub struct Omega {
    pub right: u8,
}

impl Omega {
    /// Tail reads the right byte.
    pub fn tail(&self) -> u8 {
        self.right
    }
}
"#;

    assert_eq!(
        digest_map(documented)?,
        digest_map(BASE)?,
        "documentation cannot change compiled output and must not reach any digest"
    );
    Ok(())
}

/// Documentation must be stripped at any nesting depth, not only on top-level declarations.
///
/// A doc comment on an item declared inside a function body is the case an attribute walker over the item AST
/// misses, because it never descends into bodies. Stripping at the token level has no depth to miss.
#[test]
fn doc_comments_are_stripped_at_any_nesting_depth() -> Result<(), Box<dyn Error>> {
    let plain = r#"pub fn holder() -> u8 {
    struct Inner;
    let _ = Inner;
    1
}

pub struct After;
"#;
    let documented = r#"pub fn holder() -> u8 {
    /// Documentation on an item nested inside a function body.
    struct Inner;
    let _ = Inner;
    1
}

pub struct After;
"#;

    assert_eq!(
        digest_map(documented)?,
        digest_map(plain)?,
        "doc attributes nested inside a function body must be stripped too"
    );
    Ok(())
}

/// Adding or removing `//` and `/* */` comments must move nothing.
///
/// The tokenizer drops comments before this crate sees them, so this invariant documents a property rather than
/// guarding an implementation choice. It still earns its place: it would fail immediately if the digest were ever
/// rebuilt over source text instead of tokens.
#[test]
fn line_comments_do_not_move_any_digest() -> Result<(), Box<dyn Error>> {
    let commented = r#"#![allow(dead_code)]
// A remark before anything is declared.

pub struct Alpha {
    // Inside the struct body.
    pub left: u8,
}

pub fn middle(value: u8) -> u8 {
    // Inside a function body, ahead of the expression that matters.
    value + 1 // and after it
}

/* a block comment between two declarations */

pub struct Omega {
    pub right: u8,
}

impl Omega {
    pub fn tail(&self) -> u8 {
        self.right
    }
}
"#;

    assert_eq!(
        digest_map(commented)?,
        digest_map(BASE)?,
        "comments never reach a token stream and must not reach any digest"
    );
    Ok(())
}

/// Reformatting must move nothing.
///
/// Collapsing the fixture onto one line changes every line number and byte offset in the file while leaving the
/// token sequence intact, which is the strongest cheap evidence that no span reaches a digest.
#[test]
fn reformatting_does_not_move_any_digest() -> Result<(), Box<dyn Error>> {
    let reformatted = concat!(
        "#![allow(dead_code)] pub struct Alpha { pub left : u8 , } ",
        "pub fn middle ( value : u8 ) -> u8 { value + 1 } ",
        "pub struct Omega { pub right : u8 , } ",
        "impl Omega { pub fn tail ( & self ) -> u8 { self . right } }",
    );

    assert_eq!(
        digest_map(reformatted)?,
        digest_map(BASE)?,
        "whitespace and line breaks must not reach any digest"
    );
    Ok(())
}

/// Reordering two declarations must move nothing.
///
/// `Alpha` and `middle` swap places while `Omega` and its `impl` stay put behind them, so any positional
/// contribution to the digest would show up in the two declarations that were never touched.
#[test]
fn reordering_declarations_does_not_move_any_digest() -> Result<(), Box<dyn Error>> {
    let reordered = r#"#![allow(dead_code)]

pub fn middle(value: u8) -> u8 {
    value + 1
}

pub struct Alpha {
    pub left: u8,
}

pub struct Omega {
    pub right: u8,
}

impl Omega {
    pub fn tail(&self) -> u8 {
        self.right
    }
}
"#;

    assert_eq!(
        digest_map(reordered)?,
        digest_map(BASE)?,
        "a declaration's position in its file must not reach any digest"
    );
    Ok(())
}

/// Inserting a declaration in the middle of a file must add one entry and disturb no other.
///
/// The insertion goes between `middle` and `Omega` rather than at the end, so the two declarations that follow it
/// are pushed to new lines. An insertion appended at the end would move nothing and would prove nothing.
#[test]
fn inserting_a_declaration_in_the_middle_leaves_every_other_digest_unchanged() -> Result<(), Box<dyn Error>> {
    let inserted = r#"#![allow(dead_code)]

pub struct Alpha {
    pub left: u8,
}

pub fn middle(value: u8) -> u8 {
    value + 1
}

pub const INSERTED: u8 = 7;

pub struct Omega {
    pub right: u8,
}

impl Omega {
    pub fn tail(&self) -> u8 {
        self.right
    }
}
"#;

    let before = digest_map(BASE)?;
    let after = digest_map(inserted)?;

    for (key, digest) in &before {
        assert_eq!(
            after.get(key),
            Some(digest),
            "inserting a declaration must not move `{key}`"
        );
    }
    let added: Vec<&String> = after.keys().filter(|key| !before.contains_key(*key)).collect();
    assert_eq!(added.len(), 1, "expected exactly one new entry, got {added:?}");
    assert!(
        added.iter().any(|key| key.contains("|INSERTED|")),
        "the new entry should be the inserted constant, got {added:?}"
    );
    Ok(())
}

/// Changing a function body must move that function's digest and nothing else, keeping its key intact.
///
/// The key has to survive a body change: a consumer needs to read this as "the same declaration, different digest"
/// rather than as a removal plus an addition.
#[test]
fn changing_a_function_body_moves_only_that_function() -> Result<(), Box<dyn Error>> {
    let changed = BASE.replace("value + 1", "value + 2");

    let before = single_of(BASE, None, RustDigestItemKind::Function, "middle")?;
    let after = single_of(&changed, None, RustDigestItemKind::Function, "middle")?;

    assert_ne!(
        before.digest, after.digest,
        "a body change must move the declaration's digest"
    );
    assert_eq!(
        before.key, after.key,
        "a body change must leave the declaration's key intact"
    );
    assert_eq!(
        digest_map_excluding(&changed, &["middle"])?,
        digest_map_excluding(BASE, &["middle"])?,
        "a body change must not move any other declaration"
    );
    Ok(())
}

/// Changing an associated function's body must leave its `impl` block's own entry alone.
///
/// The `impl` header entry exists so that a change to the block's generics or trait reference is visible; it must
/// not also absorb every method body, or every method in a block would appear to change together.
#[test]
fn changing_an_associated_function_body_leaves_the_impl_header_untouched() -> Result<(), Box<dyn Error>> {
    let changed = BASE.replace("self.right\n", "self.right.wrapping_add(0)\n");

    let before = single_of(BASE, Some("Omega"), RustDigestItemKind::Function, "tail")?;
    let after = single_of(&changed, Some("Omega"), RustDigestItemKind::Function, "tail")?;
    assert_ne!(before.digest, after.digest, "the method's digest must move");
    assert_eq!(before.key, after.key, "the method's key must survive a body change");

    let header_before = single_of(BASE, None, RustDigestItemKind::Impl, "Omega")?;
    let header_after = single_of(&changed, None, RustDigestItemKind::Impl, "Omega")?;
    assert_eq!(
        header_before, header_after,
        "the impl header entry must not absorb its methods' bodies"
    );
    Ok(())
}

/// Changing a struct's fields must move that struct's digest while keeping its key intact.
///
/// A named struct's braced field list is its body for keying purposes, so the key survives while the digest moves.
#[test]
fn changing_a_struct_field_moves_only_that_struct() -> Result<(), Box<dyn Error>> {
    let changed = BASE.replace("pub left: u8", "pub left: u16");

    let before = single_of(BASE, None, RustDigestItemKind::Struct, "Alpha")?;
    let after = single_of(&changed, None, RustDigestItemKind::Struct, "Alpha")?;

    assert_ne!(before.digest, after.digest, "a field type change must move the digest");
    assert_eq!(before.key, after.key, "a field type change must leave the key intact");
    assert_eq!(
        digest_map_excluding(&changed, &["Alpha"])?,
        digest_map_excluding(BASE, &["Alpha"])?,
        "a field type change must not move any other declaration"
    );
    Ok(())
}

/// Changing a function signature must move its digest and its discriminant, and nothing else.
///
/// Unlike a body change, the signature discriminant is expected to move here; the stable part of the key must not,
/// so the declaration remains findable.
#[test]
fn changing_a_function_signature_moves_only_that_function() -> Result<(), Box<dyn Error>> {
    let changed = BASE.replace("pub fn middle(value: u8)", "pub fn middle(value: u8, extra: u8)");

    let before = single_of(BASE, None, RustDigestItemKind::Function, "middle")?;
    let after = single_of(&changed, None, RustDigestItemKind::Function, "middle")?;

    assert_ne!(
        before.digest, after.digest,
        "a signature change must move the declaration's digest"
    );
    assert_ne!(
        before.key.signature_discriminant, after.key.signature_discriminant,
        "a signature change must move the discriminant that distinguishes same-named declarations"
    );
    assert_eq!(
        (
            &before.key.module_path,
            &before.key.owner,
            before.key.kind,
            &before.key.name
        ),
        (
            &after.key.module_path,
            &after.key.owner,
            after.key.kind,
            &after.key.name
        ),
        "a signature change must leave the findable part of the key intact"
    );
    assert_eq!(
        digest_map_excluding(&changed, &["middle"])?,
        digest_map_excluding(BASE, &["middle"])?,
        "a signature change must not move any other declaration"
    );
    Ok(())
}

/// Same-named methods on different owners must receive distinct keys and distinct digests.
///
/// Rust has no free-function overloading, but method names collide constantly across `impl` blocks, and an inherent
/// method and a trait method of one type collide on the same type. Without an owner in the key these would share an
/// entry and one would silently mask the other.
#[test]
fn same_named_methods_on_different_owners_stay_distinct() -> Result<(), Box<dyn Error>> {
    let source = r#"pub struct Alpha;
pub struct Omega;

pub trait Tailed {
    fn tail(&self) -> u8;
}

impl Alpha {
    pub fn tail(&self) -> u8 {
        1
    }
}

impl Tailed for Alpha {
    fn tail(&self) -> u8 {
        2
    }
}

impl Omega {
    pub fn tail(&self) -> u8 {
        3
    }
}

pub struct After;
"#;

    let digest = digest_rust_source(MODULE, source)?;
    let inherent = single(&digest, Some("Alpha"), RustDigestItemKind::Function, "tail")?;
    let via_trait = single(&digest, Some("<Alpha as Tailed>"), RustDigestItemKind::Function, "tail")?;
    let other_owner = single(&digest, Some("Omega"), RustDigestItemKind::Function, "tail")?;

    let digests = [&inherent.digest, &via_trait.digest, &other_owner.digest];
    let unique: BTreeMap<&String, ()> = digests.iter().map(|digest| (*digest, ())).collect();
    assert_eq!(unique.len(), 3, "three distinct method bodies must produce three digests");
    Ok(())
}

/// A module's inner attributes must reach its digest.
///
/// `#![deny(…)]`, `#![no_std]`, and `#![feature(…)]` change compiled output while belonging to no declaration. If
/// the crate-root entry did not carry them, editing one would be invisible, which is the under-invalidation this
/// module exists to prevent.
#[test]
fn inner_attributes_reach_the_module_digest() -> Result<(), Box<dyn Error>> {
    let changed = BASE.replace("#![allow(dead_code)]", "#![deny(dead_code)]");

    let before = digest_rust_source(MODULE, BASE)?;
    let after = digest_rust_source(MODULE, &changed)?;
    let root_before = before
        .find("", None, RustDigestItemKind::Module, MODULE)
        .first()
        .map(|item| item.digest.clone())
        .ok_or("expected a crate-root module entry")?;
    let root_after = after
        .find("", None, RustDigestItemKind::Module, MODULE)
        .first()
        .map(|item| item.digest.clone())
        .ok_or("expected a crate-root module entry")?;

    assert_ne!(
        root_before, root_after,
        "a non-doc inner attribute change must move the module digest"
    );
    assert_eq!(
        digest_map_excluding(&changed, &[MODULE])?,
        digest_map_excluding(BASE, &[MODULE])?,
        "an inner attribute change must not move any declaration"
    );
    Ok(())
}

/// Punctuation spacing must follow meaning, not layout.
///
/// Spacing is the only thing separating `a && b`, a conjunction, from `a & &b`, a bitwise `and` against a
/// reference, so it cannot be discarded without under-invalidating. It also cannot be taken from the lexer, which
/// would make `&&u8` and `& &u8` — one type, two layouts — digest differently. Both halves hold here because the
/// digest runs over the stream `syn` re-emits from the parsed item, and that round trip normalizes spacing onto
/// meaning rather than onto how the author typed it. Both halves are asserted, because an implementation that
/// hashed the lexed stream instead would still satisfy the second one alone.
#[test]
fn punctuation_spacing_follows_meaning_not_layout() -> Result<(), Box<dyn Error>> {
    // ---- Layout: one type, written two ways ----
    let tight = "pub fn probe(x: &&u8) -> u8 {\n    **x\n}\n\npub struct After;\n";
    let loose = "pub fn probe(x: & &u8) -> u8 {\n    **x\n}\n\npub struct After;\n";
    assert_eq!(
        digest_map(tight)?,
        digest_map(loose)?,
        "a double reference must digest the same however its ampersands are spaced"
    );

    // ---- Meaning: two operations, separated by nothing but spacing ----
    let conjunction = "pub fn probe(a: bool, b: bool) -> bool {\n    a && b\n}\n\npub struct After;\n";
    let bitand_ref = "pub fn probe(a: bool, b: bool) -> bool {\n    a & &b\n}\n\npub struct After;\n";
    assert_ne!(
        single_of(conjunction, None, RustDigestItemKind::Function, "probe")?.digest,
        single_of(bitand_ref, None, RustDigestItemKind::Function, "probe")?.digest,
        "`a && b` and `a & &b` are different operations and must not share a digest"
    );
    assert_eq!(
        single_of(conjunction, None, RustDigestItemKind::Struct, "After")?,
        single_of(bitand_ref, None, RustDigestItemKind::Struct, "After")?,
        "the declaration after the edit must be untouched by it"
    );
    Ok(())
}

/// Doc attributes inside a macro body are retained, because there they may not be documentation at all.
///
/// Outside macro token soup a `#[…]` is always an attribute, which is what makes recognizing doc attributes by shape
/// exact. Inside a macro body that guarantee is gone, so the body is hashed verbatim and editing a doc comment there
/// over-invalidates. That is the safe direction and is asserted here so the carve-out cannot be removed unnoticed.
#[test]
fn doc_attributes_inside_macro_bodies_are_retained() -> Result<(), Box<dyn Error>> {
    let original = r#"macro_rules! declare {
    () => {
        /// original documentation
        pub struct Generated;
    };
}

pub struct After;
"#;
    let revised = original.replace("original documentation", "revised documentation");

    let before = single_of(original, None, RustDigestItemKind::Macro, "declare")?;
    let after = single_of(&revised, None, RustDigestItemKind::Macro, "declare")?;
    assert_ne!(
        before.digest, after.digest,
        "macro bodies are hashed verbatim, so a doc comment inside one is expected to move the digest"
    );

    assert_eq!(
        single_of(original, None, RustDigestItemKind::Struct, "After")?,
        single_of(&revised, None, RustDigestItemKind::Struct, "After")?,
        "the declaration after the macro must be untouched by it"
    );
    Ok(())
}

/// A file that does not parse is reported, never partially digested.
///
/// A partial digest of a broken file is indistinguishable from a complete one to a consumer and would hide whatever
/// followed the syntax error.
#[test]
fn unparseable_source_is_reported_rather_than_partially_digested() -> Result<(), Box<dyn Error>> {
    let outcome = digest_rust_source(MODULE, "pub fn broken( {");
    assert!(
        matches!(outcome, Err(RustDigestError::Parse { .. })),
        "expected a parse error, got {outcome:?}"
    );
    Ok(())
}


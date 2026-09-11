//! Semantic digests for the Rust declarations an Incan build links against.
//!
//! A build-identity closure that folds only Incan declarations is not partially correct, it is unsound: a change to
//! the Rust runtime an Incan component links against produces a cache hit for a change the consumer could not see.
//! This module closes that gap for Rust source by giving every declaration a digest that moves whenever any token in
//! it moves.
//!
//! # What this is, and what it is not
//!
//! This is the token-stream stopgap, chosen deliberately over waiting for body lowering. It over-invalidates inside a
//! file — reordering a function's local statements moves that function's digest even when the compiled output is
//! identical — and it under-invalidates nowhere. Over-invalidating costs build time; under-invalidating yields a
//! wrong binary, so the asymmetry decides the design.
//!
//! It is a digest of a *source file's tokens*. It does not resolve names, expand macros, or read anything the file
//! does not contain. A change reaching this file only through a macro defined elsewhere, through an `include!`, or
//! through a build script is invisible here and must be covered by digesting those inputs too.
//!
//! # Keys
//!
//! A digest is only useful next to a stable name for what it digests. [`RustItemDigestKey`] pairs the declaration's
//! module path, owner, kind, and name with a *signature discriminant*: the digest of the declaration's header. The
//! discriminant is what stops `Alpha::tail` and `Omega::tail`, or two `#[cfg]`-gated spellings of one function, from
//! sharing a key. Because the header excludes a trailing brace-delimited body where one exists, an item's key
//! survives edits to its body, which is what lets a consumer say "this item changed" rather than "an item vanished
//! and another appeared".

use proc_macro2::{Delimiter, TokenStream, TokenTree};
use quote::ToTokens;
use serde::{Deserialize, Serialize};
use syn::{ForeignItem, ImplItem, Item, ItemForeignMod, ItemImpl, ItemMod, ItemTrait, TraitItem};

use crate::digest_tokens::digest_tokens;

/// Failure modes for Rust declaration digesting.
#[derive(Debug, thiserror::Error)]
pub enum RustDigestError {
    /// The supplied text is not a parseable Rust source file.
    ///
    /// Digesting is all-or-nothing per file on purpose. A partial digest of a file that failed to parse would look
    /// like a complete one to a consumer and would silently hide whatever followed the syntax error.
    #[error("failed to parse Rust source for digesting: {message}")]
    Parse {
        /// The parser's message, without position information.
        message: String,
    },
}

/// The kind of Rust declaration a digest describes.
///
/// The kind is part of the key because Rust keeps types and values in separate namespaces, so `struct Handle` and
/// `fn Handle` can coexist in one module and must not share an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum RustDigestItemKind {
    /// A free, associated, trait, or `extern` block function.
    Function,
    /// A `struct` declaration.
    Struct,
    /// An `enum` declaration.
    Enum,
    /// A `union` declaration.
    Union,
    /// A `type` alias, including an associated type in a trait or `impl`.
    TypeAlias,
    /// A `const` item, including an associated constant.
    Const,
    /// A `static` item, including one declared in an `extern` block.
    Static,
    /// A `trait` declaration's header; its associated items get their own entries.
    Trait,
    /// A `trait` alias.
    TraitAlias,
    /// An `impl` block's header; its associated items get their own entries.
    Impl,
    /// A module's header and inner attributes; its items get their own entries.
    Module,
    /// A `use` declaration.
    Use,
    /// A macro definition or an item-position macro invocation.
    Macro,
    /// An `extern crate` declaration.
    ExternCrate,
    /// An `extern` block's header; its items get their own entries.
    ForeignBlock,
    /// A declaration this crate does not model individually, digested as raw tokens.
    Verbatim,
}

/// A stable identity for one Rust declaration.
///
/// Ordering is defined so a whole file's entries can be compared as a sorted sequence, which is what makes reordering
/// declarations in a file a no-op for the digest set.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RustItemDigestKey {
    /// The module path containing this declaration, as supplied by the caller and extended by inline `mod` blocks.
    pub module_path: String,
    /// The `impl`, `trait`, or `extern` block owning this declaration, rendered from tokens; `None` at module level.
    pub owner: Option<String>,
    /// Which kind of declaration this is.
    pub kind: RustDigestItemKind,
    /// The declaration's name, or a rendered stand-in for the declarations that have none.
    pub name: String,
    /// Digest of the declaration's header, distinguishing same-named declarations that differ before their body.
    pub signature_discriminant: String,
}

impl RustItemDigestKey {
    /// Render the key as one line, for diagnostics and for use as a map key.
    ///
    /// The rendering is lossless with respect to the key's fields, so two keys render identically only when they are
    /// equal.
    pub fn render(&self) -> String {
        let owner = self.owner.as_deref().unwrap_or("");
        format!(
            "{}|{}|{:?}|{}|{}",
            self.module_path, owner, self.kind, self.name, self.signature_discriminant
        )
    }
}

/// One Rust declaration's identity and semantic digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustItemDigest {
    /// Stable identity for this declaration.
    pub key: RustItemDigestKey,
    /// Lowercase hexadecimal SHA-256 over the declaration's tokens, doc attributes excluded.
    pub digest: String,
}

/// Every declaration digest extracted from one Rust source file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustSourceDigest {
    items: Vec<RustItemDigest>,
}

impl RustSourceDigest {
    /// All declarations, in a deterministic order that does not depend on their order in the source file.
    pub fn items(&self) -> &[RustItemDigest] {
        &self.items
    }

    /// Take ownership of the declarations.
    pub fn into_items(self) -> Vec<RustItemDigest> {
        self.items
    }

    /// Find the declarations matching an identity, ignoring only the signature discriminant.
    ///
    /// Callers look declarations up by the part of the key that survives an edit, so a signature change reads as "the
    /// same declaration, different digest" rather than as a removal plus an addition. The kind is part of the lookup
    /// because a type and its `impl` block legitimately share a name; more than one match after that means the name
    /// is genuinely repeated, as `#[cfg]`-gated spellings of one declaration are, and the caller decides what that
    /// means.
    pub fn find(
        &self,
        module_path: &str,
        owner: Option<&str>,
        kind: RustDigestItemKind,
        name: &str,
    ) -> Vec<&RustItemDigest> {
        self.items
            .iter()
            .filter(|item| {
                item.key.module_path == module_path
                    && item.key.owner.as_deref() == owner
                    && item.key.kind == kind
                    && item.key.name == name
            })
            .collect()
    }
}

/// Digest every declaration in one Rust source file.
///
/// `module_path` names the module the file *is*, for example `incan_stdlib::runtime`; it seeds the module path of
/// every declaration and is not otherwise interpreted. Inline `mod` blocks extend it.
///
/// The file itself contributes a [`RustDigestItemKind::Module`] entry carrying its inner attributes, so a change to
/// `#![no_std]` or `#![feature(…)]` is visible even though it belongs to no declaration.
pub fn digest_rust_source(module_path: &str, source: &str) -> Result<RustSourceDigest, RustDigestError> {
    let file = syn::parse_file(source).map_err(|error| RustDigestError::Parse {
        message: error.to_string(),
    })?;
    let mut walker = DigestWalker::default();
    walker.walk_file(module_path, &file);
    let mut items = walker.items;
    items.sort_by(|left, right| left.key.cmp(&right.key));
    Ok(RustSourceDigest { items })
}

/// Accumulator threaded through the declaration walk.
#[derive(Default)]
struct DigestWalker {
    items: Vec<RustItemDigest>,
}

impl DigestWalker {
    /// Record one declaration's key and digest.
    fn record(
        &mut self,
        module_path: &str,
        owner: Option<&str>,
        kind: RustDigestItemKind,
        name: impl Into<String>,
        tokens: TokenStream,
    ) {
        let header = header_tokens(&tokens);
        self.items.push(RustItemDigest {
            key: RustItemDigestKey {
                module_path: module_path.to_string(),
                owner: owner.map(str::to_string),
                kind,
                name: name.into(),
                signature_discriminant: digest_tokens(header),
            },
            digest: digest_tokens(tokens),
        });
    }

    /// Walk a parsed file, recording its inner attributes as a module entry before descending into its items.
    fn walk_file(&mut self, module_path: &str, file: &syn::File) {
        let (parent, name) = split_module_path(module_path);
        let mut attrs = TokenStream::new();
        for attr in &file.attrs {
            attr.to_tokens(&mut attrs);
        }
        self.record(parent, None, RustDigestItemKind::Module, name, attrs);
        self.walk_items(module_path, &file.items);
    }

    /// Walk the items of one module scope.
    fn walk_items(&mut self, module_path: &str, items: &[Item]) {
        for item in items {
            self.walk_item(module_path, item);
        }
    }

    /// Record one module-level item, descending into the scopes that contain further declarations.
    ///
    /// `Item` is `#[non_exhaustive]`, so an unrecognized future variant falls back to a verbatim token digest rather
    /// than being dropped; dropping it would be the under-invalidation this module exists to avoid.
    fn walk_item(&mut self, module_path: &str, item: &Item) {
        use RustDigestItemKind as Kind;
        match item {
            Item::Fn(item) => self.record(
                module_path,
                None,
                Kind::Function,
                item.sig.ident.to_string(),
                item.to_token_stream(),
            ),
            Item::Struct(item) => self.record(
                module_path,
                None,
                Kind::Struct,
                item.ident.to_string(),
                item.to_token_stream(),
            ),
            Item::Enum(item) => self.record(
                module_path,
                None,
                Kind::Enum,
                item.ident.to_string(),
                item.to_token_stream(),
            ),
            Item::Union(item) => self.record(
                module_path,
                None,
                Kind::Union,
                item.ident.to_string(),
                item.to_token_stream(),
            ),
            Item::Type(item) => self.record(
                module_path,
                None,
                Kind::TypeAlias,
                item.ident.to_string(),
                item.to_token_stream(),
            ),
            Item::Const(item) => self.record(
                module_path,
                None,
                Kind::Const,
                item.ident.to_string(),
                item.to_token_stream(),
            ),
            Item::Static(item) => self.record(
                module_path,
                None,
                Kind::Static,
                item.ident.to_string(),
                item.to_token_stream(),
            ),
            Item::TraitAlias(item) => self.record(
                module_path,
                None,
                Kind::TraitAlias,
                item.ident.to_string(),
                item.to_token_stream(),
            ),
            Item::ExternCrate(item) => self.record(
                module_path,
                None,
                Kind::ExternCrate,
                item.ident.to_string(),
                item.to_token_stream(),
            ),
            Item::Use(item) => self.record(module_path, None, Kind::Use, "use", item.to_token_stream()),
            Item::Macro(item) => {
                let name = item
                    .ident
                    .as_ref()
                    .map_or_else(|| render_tokens(&item.mac.path), ToString::to_string);
                self.record(module_path, None, Kind::Macro, name, item.to_token_stream());
            }
            Item::Mod(item) => self.walk_module(module_path, item),
            Item::Trait(item) => self.walk_trait(module_path, item),
            Item::Impl(item) => self.walk_impl(module_path, item),
            Item::ForeignMod(item) => self.walk_foreign_mod(module_path, item),
            other => self.record(module_path, None, Kind::Verbatim, "", other.to_token_stream()),
        }
    }

    /// Record a module's header and inner attributes, then descend into its items.
    ///
    /// The module's own entry deliberately excludes its items: they are recorded individually, and folding them in
    /// twice would make every entry in a file move whenever any one of them did.
    fn walk_module(&mut self, module_path: &str, item: &ItemMod) {
        let name = item.ident.to_string();
        let mut shell = item.clone();
        shell.content = shell.content.map(|(brace, _)| (brace, Vec::new()));
        self.record(
            module_path,
            None,
            RustDigestItemKind::Module,
            name.clone(),
            shell.to_token_stream(),
        );
        if let Some((_, items)) = &item.content {
            self.walk_items(&join_module_path(module_path, &name), items);
        }
    }

    /// Record a trait's header, then each of its associated items under the trait as owner.
    fn walk_trait(&mut self, module_path: &str, item: &ItemTrait) {
        let name = item.ident.to_string();
        let mut shell = item.clone();
        shell.items = Vec::new();
        self.record(
            module_path,
            None,
            RustDigestItemKind::Trait,
            name.clone(),
            shell.to_token_stream(),
        );
        for assoc in &item.items {
            self.walk_trait_item(module_path, &name, assoc);
        }
    }

    /// Record one associated item of a trait.
    fn walk_trait_item(&mut self, module_path: &str, owner: &str, item: &TraitItem) {
        use RustDigestItemKind as Kind;
        let owner = Some(owner);
        match item {
            TraitItem::Fn(item) => self.record(
                module_path,
                owner,
                Kind::Function,
                item.sig.ident.to_string(),
                item.to_token_stream(),
            ),
            TraitItem::Const(item) => self.record(
                module_path,
                owner,
                Kind::Const,
                item.ident.to_string(),
                item.to_token_stream(),
            ),
            TraitItem::Type(item) => self.record(
                module_path,
                owner,
                Kind::TypeAlias,
                item.ident.to_string(),
                item.to_token_stream(),
            ),
            TraitItem::Macro(item) => self.record(
                module_path,
                owner,
                Kind::Macro,
                render_tokens(&item.mac.path),
                item.to_token_stream(),
            ),
            other => self.record(module_path, owner, Kind::Verbatim, "", other.to_token_stream()),
        }
    }

    /// Record an `impl` block's header, then each of its associated items under the impl target as owner.
    fn walk_impl(&mut self, module_path: &str, item: &ItemImpl) {
        let owner = impl_owner(item);
        let mut shell = item.clone();
        shell.items = Vec::new();
        self.record(
            module_path,
            None,
            RustDigestItemKind::Impl,
            owner.clone(),
            shell.to_token_stream(),
        );
        for assoc in &item.items {
            self.walk_impl_item(module_path, &owner, assoc);
        }
    }

    /// Record one associated item of an `impl` block.
    fn walk_impl_item(&mut self, module_path: &str, owner: &str, item: &ImplItem) {
        use RustDigestItemKind as Kind;
        let owner = Some(owner);
        match item {
            ImplItem::Fn(item) => self.record(
                module_path,
                owner,
                Kind::Function,
                item.sig.ident.to_string(),
                item.to_token_stream(),
            ),
            ImplItem::Const(item) => self.record(
                module_path,
                owner,
                Kind::Const,
                item.ident.to_string(),
                item.to_token_stream(),
            ),
            ImplItem::Type(item) => self.record(
                module_path,
                owner,
                Kind::TypeAlias,
                item.ident.to_string(),
                item.to_token_stream(),
            ),
            ImplItem::Macro(item) => self.record(
                module_path,
                owner,
                Kind::Macro,
                render_tokens(&item.mac.path),
                item.to_token_stream(),
            ),
            other => self.record(module_path, owner, Kind::Verbatim, "", other.to_token_stream()),
        }
    }

    /// Record an `extern` block's header, then each of its declarations under that block as owner.
    fn walk_foreign_mod(&mut self, module_path: &str, item: &ItemForeignMod) {
        let owner = render_tokens(&item.abi);
        let mut shell = item.clone();
        shell.items = Vec::new();
        self.record(
            module_path,
            None,
            RustDigestItemKind::ForeignBlock,
            owner.clone(),
            shell.to_token_stream(),
        );
        for foreign in &item.items {
            self.walk_foreign_item(module_path, &owner, foreign);
        }
    }

    /// Record one declaration inside an `extern` block.
    fn walk_foreign_item(&mut self, module_path: &str, owner: &str, item: &ForeignItem) {
        use RustDigestItemKind as Kind;
        let owner = Some(owner);
        match item {
            ForeignItem::Fn(item) => self.record(
                module_path,
                owner,
                Kind::Function,
                item.sig.ident.to_string(),
                item.to_token_stream(),
            ),
            ForeignItem::Static(item) => self.record(
                module_path,
                owner,
                Kind::Static,
                item.ident.to_string(),
                item.to_token_stream(),
            ),
            ForeignItem::Type(item) => self.record(
                module_path,
                owner,
                Kind::TypeAlias,
                item.ident.to_string(),
                item.to_token_stream(),
            ),
            ForeignItem::Macro(item) => self.record(
                module_path,
                owner,
                Kind::Macro,
                render_tokens(&item.mac.path),
                item.to_token_stream(),
            ),
            other => self.record(module_path, owner, Kind::Verbatim, "", other.to_token_stream()),
        }
    }
}

/// Split a declaration's header from its brace-delimited body.
///
/// Functions, named structs, enums, unions, traits, `impl` blocks, and inline modules all end in a braced body, so
/// dropping the final brace group leaves exactly the header a reader would call the declaration's signature. Items
/// that end in `;` instead — constants, statics, aliases, tuple structs, `use` — have no body to separate, so their
/// header is the whole item and their key moves when their value does. That is over-invalidation, and for a constant
/// it is arguably the truth: its value is part of what callers compile against.
fn header_tokens(tokens: &TokenStream) -> TokenStream {
    let trees: Vec<TokenTree> = tokens.clone().into_iter().collect();
    match trees.last() {
        Some(TokenTree::Group(group)) if group.delimiter() == Delimiter::Brace => {
            trees.iter().take(trees.len() - 1).cloned().collect()
        }
        _ => tokens.clone(),
    }
}

/// Render an `impl` block's target as the owner string its associated items are keyed under.
///
/// A trait implementation renders as `<SelfType as Trait>` so that inherent and trait methods of the same name stay
/// distinct, and so that two traits contributing the same method name to one type do too. A negative implementation
/// keeps its `!`, because `impl !Send for X` and `impl Send for X` are different facts.
fn impl_owner(item: &ItemImpl) -> String {
    let self_ty = render_tokens(&item.self_ty);
    match &item.trait_ {
        Some((negation, path, _)) => {
            let negation = if negation.is_some() { "!" } else { "" };
            format!("<{self_ty} as {negation}{}>", render_tokens(path))
        }
        None => self_ty,
    }
}

/// Render syntax as its token text, for the parts of a key that must read back to a human.
///
/// Rendering goes through tokens rather than source text so that `Vec<T>` and `Vec < T >` produce the same owner, in
/// keeping with the rest of the digest. This feeds keys only, never digests, so its exact spelling is a readability
/// concern rather than a correctness one.
fn render_tokens(value: &impl ToTokens) -> String {
    value.to_token_stream().to_string()
}

/// Split a module path into its parent path and its own final segment.
fn split_module_path(module_path: &str) -> (&str, &str) {
    module_path.rsplit_once("::").unwrap_or(("", module_path))
}

/// Join a parent module path with a child segment, tolerating an empty parent.
fn join_module_path(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_string()
    } else {
        format!("{parent}::{child}")
    }
}

#[cfg(test)]
mod tests {
    include!("digest/tests.rs");
}

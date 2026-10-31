//! Checked diagnostics stored as semantic values: a position-free catalog recipe and structural anchors.
//!
//! An anchor names its owner declaration by category, declared name and same-name ordinal, carries the owner's
//! normalized syntax root, and selects a node inside the owner by a structural path of field, variant and element
//! roles. The root is a digest of the owner's syntax with every source coordinate and formatter layout removed and
//! expansion identity kept, so a comment or blank-line edit leaves owner, root and path equal. Source coordinates of
//! the current revision live only in [`DiagnosticPositions`], which reads the checked syntax when a value is
//! captured or rendered and never re-enters the checker.

use std::collections::BTreeMap;
use std::fmt::Display;

use serde::{Serialize, ser};
use sha2::{Digest, Sha256};

use crate::ast::{Declaration, Program, Span, Spanned};
use crate::diagnostics::{CompileError, DiagnosticRanges, DiagnosticRecipe, ErrorKind};
use incan_semantics_core::SymbolOrigin;

/// A stored diagnostic: the catalog recipe it was built from, without coordinates, and one anchor per source range
/// it carries. The recipe keeps the stable code selection, so the catalog code is a function of the stored value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AnchoredDiagnostic {
    recipe: DiagnosticRecipe,
    primary: SourceAnchor,
    related: Vec<SourceAnchor>,
    declarations: Vec<DeclarationSite>,
}

impl AnchoredDiagnostic {
    /// Severity and category of the stored value, readable without binding it to a revision.
    pub(crate) fn kind(&self) -> ErrorKind {
        self.recipe.kind()
    }
}

/// Owner declaration of an anchor, independent of where the declaration sits in its file.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct OwnerKey {
    category: &'static str,
    name: Option<String>,
    /// Position among declarations of the same category and name, so overloads stay distinct.
    ordinal: usize,
}

/// One syntax node: the owner, the owner's normalized root and the structural path to the node's range.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NodeAnchor {
    owner: OwnerKey,
    root: String,
    path: String,
}

/// Which end of a node's range a boundary selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edge {
    Start,
    End,
}

/// A structural anchor for one source range. Only [`SourceAnchor::Detached`] holds coordinates, for a range that
/// lies outside every declaration; capture counts it so it cannot pass unobserved.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SourceAnchor {
    /// Exactly one node's range.
    Node(NodeAnchor),
    /// From one node boundary to another, for ranges joined from several nodes.
    Between {
        start: (NodeAnchor, Edge),
        end: (NodeAnchor, Edge),
        expansion: Option<[u8; 32]>,
    },
    /// Inside the smallest enclosing node, at node-relative offsets. The node length is a layout witness: a binding
    /// whose node length differs is refused instead of rendering a stale offset.
    Within {
        node: NodeAnchor,
        start: usize,
        end: usize,
        length: usize,
        expansion: Option<[u8; 32]>,
    },
    /// The empty default range of a diagnostic without a source location.
    Unlocated,
    /// A range outside every declaration of the checked file.
    Detached(Span),
}

/// The declaration site of a related declaration: anchored when it belongs to the checked file, otherwise kept in
/// its own provider's coordinates, which no edit to this file can move.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DeclarationSite {
    Local(SourceAnchor),
    Provider(Span),
}

/// Why a stored value cannot be bound to the current syntax. None of these is a source refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AnchorFault {
    /// The owner declaration no longer exists in the current syntax.
    MissingOwner,
    /// The owner's normalized syntax root changed, so its structural paths no longer correspond.
    ChangedOwner,
    /// The structural path selects no range in the current owner.
    MissingNode,
    /// A node-relative range no longer fits its node's current layout.
    ChangedLayout,
    /// The recipe and the bound ranges disagree on the number of supplements.
    SupplementMismatch,
    /// The owner's syntax could not be serialized.
    Encoding(String),
}

/// Revision-bound positions of one checked program: owner keys for every declaration and, built on first use, each
/// owner's normalized root and structural range inventory.
pub(crate) struct DiagnosticPositions<'program> {
    program: &'program Program,
    owners: Vec<OwnerKey>,
    owner_index: BTreeMap<OwnerKey, usize>,
    syntax: BTreeMap<usize, OwnerSyntax>,
    origin: SymbolOrigin,
    /// Values stored since this binding was created.
    stored: usize,
    /// Ranges outside every declaration, captured since this binding was created.
    detached: usize,
    /// Node-relative ranges captured since this binding was created.
    within: usize,
}

/// Capture observations of one binding; activation-local counters, never a reuse authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CaptureCounts {
    pub(crate) stored: usize,
    pub(crate) within: usize,
    pub(crate) detached: usize,
}

/// One owner's normalized syntax root and the range of every syntax node, keyed both ways.
struct OwnerSyntax {
    root: String,
    by_path: BTreeMap<String, Span>,
    by_span: BTreeMap<Span, String>,
    starts: BTreeMap<(usize, Option<[u8; 32]>), String>,
    ends: BTreeMap<(usize, Option<[u8; 32]>), String>,
}

impl<'program> DiagnosticPositions<'program> {
    /// Bind the positions of `program`, the exact syntax the checker checked. `origin` is the checked module's
    /// declaration origin, which decides whether a related declaration's site belongs to this file.
    pub(crate) fn new(program: &'program Program, origin: SymbolOrigin) -> Self {
        let mut counts = BTreeMap::new();
        let owners = program
            .declarations
            .iter()
            .map(|declaration| {
                let (category, name) = owner_name(declaration);
                let ordinal = counts.entry((category, name.clone())).or_insert(0usize);
                let key = OwnerKey {
                    category,
                    name,
                    ordinal: *ordinal,
                };
                *ordinal += 1;
                key
            })
            .collect::<Vec<_>>();
        let owner_index = owners
            .iter()
            .enumerate()
            .map(|(index, key)| (key.clone(), index))
            .collect();
        Self {
            program,
            owners,
            owner_index,
            syntax: BTreeMap::new(),
            origin,
            stored: 0,
            detached: 0,
            within: 0,
        }
    }

    /// Values stored and the anchors that needed node-relative offsets or kept coordinates, since this binding began.
    pub(crate) fn capture_counts(&self) -> CaptureCounts {
        CaptureCounts {
            stored: self.stored,
            within: self.within,
            detached: self.detached,
        }
    }

    /// Store one checked diagnostic as a semantic value. The capture is total for the checked revision: every range
    /// receives an anchor, so no positioned diagnostic survives beside the stored value.
    pub(crate) fn capture(&mut self, error: CompileError) -> Result<AnchoredDiagnostic, AnchorFault> {
        let (recipe, ranges) = error.into_recipe();
        let primary = self.anchor(ranges.primary)?;
        let related = ranges
            .related
            .iter()
            .map(|span| self.anchor(*span))
            .collect::<Result<Vec<_>, _>>()?;
        let declarations = recipe
            .related_declarations()
            .iter()
            .zip(&ranges.declarations)
            .map(|(related, span)| {
                if related.identity.origin == self.origin {
                    self.anchor(*span).map(DeclarationSite::Local)
                } else {
                    Ok(DeclarationSite::Provider(*span))
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(AnchoredDiagnostic {
            recipe,
            primary,
            related,
            declarations,
        })
    }

    /// Bind a stored value to this revision's ranges. Rendering reads only the syntax inventory; it never checks.
    pub(crate) fn render(&mut self, value: &AnchoredDiagnostic) -> Result<CompileError, AnchorFault> {
        let ranges = DiagnosticRanges {
            primary: self.resolve(&value.primary)?,
            related: value
                .related
                .iter()
                .map(|anchor| self.resolve(anchor))
                .collect::<Result<Vec<_>, _>>()?,
            declarations: value
                .declarations
                .iter()
                .map(|site| match site {
                    DeclarationSite::Local(anchor) => self.resolve(anchor),
                    DeclarationSite::Provider(span) => Ok(*span),
                })
                .collect::<Result<Vec<_>, _>>()?,
        };
        value.recipe.diagnostic(&ranges).ok_or(AnchorFault::SupplementMismatch)
    }

    /// Store a diagnostic of the checked revision. Capture is total over this revision's syntax, so a fault means
    /// the checked syntax cannot be serialized: an internal inconsistency, never a source refusal.
    pub(crate) fn store(&mut self, error: CompileError) -> AnchoredDiagnostic {
        self.stored += 1;
        self.capture(error)
            .unwrap_or_else(|fault| panic!("a checked diagnostic has no structural anchor: {fault:?}"))
    }

    /// Publish a stored value whose applicability to this revision is established: it was stored from this revision,
    /// or its caller validated the reuse. A fault is an internal inconsistency, never a source refusal.
    pub(crate) fn publish(&mut self, value: &AnchoredDiagnostic) -> CompileError {
        self.render(value)
            .unwrap_or_else(|fault| panic!("a stored diagnostic does not bind to its revision: {fault:?}"))
    }

    // ---- Anchors ----

    /// Select the structural anchor for one range: an exact node, a pair of node boundaries, or a node-relative
    /// range, searching the declarations that contain it first and every other declaration after. The empty default
    /// range means "no location" and is never bound to a node.
    fn anchor(&mut self, span: Span) -> Result<SourceAnchor, AnchorFault> {
        if span == Span::default() {
            return Ok(SourceAnchor::Unlocated);
        }
        let candidates = self.candidates(span);
        for &index in &candidates {
            if let Some(path) = self.owner_syntax(index)?.by_span.get(&span).cloned() {
                return Ok(SourceAnchor::Node(self.node(index, path)?));
            }
        }
        for &index in &candidates {
            let syntax = self.owner_syntax(index)?;
            let start = boundary(syntax, span.start, span.expansion);
            let end = boundary(syntax, span.end, span.expansion);
            if let (Some(start), Some(end)) = (start, end) {
                return Ok(SourceAnchor::Between {
                    start: (self.node(index, start.0)?, start.1),
                    end: (self.node(index, end.0)?, end.1),
                    expansion: span.expansion,
                });
            }
        }
        for &index in &candidates {
            let enclosing = self
                .owner_syntax(index)?
                .by_path
                .iter()
                .filter(|(_, node)| {
                    node.expansion == span.expansion && node.start <= span.start && span.end <= node.end
                })
                .min_by_key(|(_, node)| node.end - node.start)
                .map(|(path, node)| (path.clone(), *node));
            if let Some((path, node)) = enclosing {
                self.within += 1;
                return Ok(SourceAnchor::Within {
                    node: self.node(index, path)?,
                    start: span.start - node.start,
                    end: span.end - node.start,
                    length: node.end - node.start,
                    expansion: span.expansion,
                });
            }
        }
        self.detached += 1;
        Ok(SourceAnchor::Detached(span))
    }

    /// Declarations whose range contains `span`, followed by every other declaration in source order.
    fn candidates(&self, span: Span) -> Vec<usize> {
        let declarations = &self.program.declarations;
        let contains = |declaration: &Spanned<Declaration>| {
            declaration.span.start <= span.start && span.end <= declaration.span.end
        };
        let mut candidates = (0..declarations.len())
            .filter(|index| contains(&declarations[*index]))
            .collect::<Vec<_>>();
        candidates.extend((0..declarations.len()).filter(|index| !contains(&declarations[*index])));
        candidates
    }

    /// Name a node of one owner with that owner's current root.
    fn node(&mut self, index: usize, path: String) -> Result<NodeAnchor, AnchorFault> {
        let root = self.owner_syntax(index)?.root.clone();
        Ok(NodeAnchor {
            owner: self.owners[index].clone(),
            root,
            path,
        })
    }

    /// Resolve one anchor in this revision.
    fn resolve(&mut self, anchor: &SourceAnchor) -> Result<Span, AnchorFault> {
        match anchor {
            SourceAnchor::Node(node) => self.resolve_node(node),
            SourceAnchor::Between { start, end, expansion } => {
                let first = self.resolve_node(&start.0)?;
                let last = self.resolve_node(&end.0)?;
                let edge = |span: Span, edge: Edge| match edge {
                    Edge::Start => span.start,
                    Edge::End => span.end,
                };
                Ok(Span::new(edge(first, start.1), edge(last, end.1)).with_expansion(*expansion))
            }
            SourceAnchor::Within {
                node,
                start,
                end,
                length,
                expansion,
            } => {
                let current = self.resolve_node(node)?;
                if current.end - current.start != *length {
                    return Err(AnchorFault::ChangedLayout);
                }
                Ok(Span::new(current.start + start, current.start + end).with_expansion(*expansion))
            }
            SourceAnchor::Unlocated => Ok(Span::default()),
            SourceAnchor::Detached(span) => Ok(*span),
        }
    }

    /// Resolve one node anchor after checking that its owner exists and still has the same normalized root.
    fn resolve_node(&mut self, node: &NodeAnchor) -> Result<Span, AnchorFault> {
        let index = *self.owner_index.get(&node.owner).ok_or(AnchorFault::MissingOwner)?;
        let syntax = self.owner_syntax(index)?;
        if syntax.root != node.root {
            return Err(AnchorFault::ChangedOwner);
        }
        syntax.by_path.get(&node.path).copied().ok_or(AnchorFault::MissingNode)
    }

    /// Build one owner's inventory on first use; owners without diagnostics are never serialized.
    fn owner_syntax(&mut self, index: usize) -> Result<&OwnerSyntax, AnchorFault> {
        if !self.syntax.contains_key(&index) {
            let syntax = OwnerSyntax::of(&self.program.declarations[index])?;
            self.syntax.insert(index, syntax);
        }
        Ok(&self.syntax[&index])
    }
}

/// The node boundary at `offset`, preferring a node start over a node end, each first in serialization order.
fn boundary(syntax: &OwnerSyntax, offset: usize, expansion: Option<[u8; 32]>) -> Option<(String, Edge)> {
    syntax
        .starts
        .get(&(offset, expansion))
        .map(|path| (path.clone(), Edge::Start))
        .or_else(|| {
            syntax
                .ends
                .get(&(offset, expansion))
                .map(|path| (path.clone(), Edge::End))
        })
}

/// Declaration category and declared name; imports and docstrings have no name of their own.
fn owner_name(declaration: &Spanned<Declaration>) -> (&'static str, Option<String>) {
    match &declaration.node {
        Declaration::Import(_) => ("import", None),
        Declaration::Const(item) => ("const", Some(item.name.clone())),
        Declaration::Static(item) => ("static", Some(item.name.clone())),
        Declaration::Model(item) => ("model", Some(item.name.clone())),
        Declaration::Capability(item) => ("capability", Some(item.name.clone())),
        Declaration::Class(item) => ("class", Some(item.name.clone())),
        Declaration::Trait(item) => ("trait", Some(item.name.clone())),
        Declaration::Alias(item) => ("alias", Some(item.name.clone())),
        Declaration::Partial(item) => ("partial", Some(item.name.clone())),
        Declaration::TypeAlias(item) => ("type", Some(item.name.clone())),
        Declaration::Newtype(item) => ("newtype", Some(item.name.clone())),
        Declaration::Enum(item) => ("enum", Some(item.name.clone())),
        Declaration::Function(item) => ("def", Some(item.name.clone())),
        Declaration::TestModule(item) => ("tests", Some(item.name.clone())),
        Declaration::VocabBlock(item) => ("vocab", Some(item.keyword.clone())),
        Declaration::Docstring(_) => ("docstring", None),
    }
}

impl OwnerSyntax {
    /// Serialize one declaration once, recording each range with its structural path and hashing everything else.
    fn of(declaration: &Spanned<Declaration>) -> Result<Self, AnchorFault> {
        let mut inventory = SyntaxInventory::default();
        declaration
            .serialize(&mut inventory)
            .map_err(|error| AnchorFault::Encoding(error.0))?;
        let mut syntax = Self {
            root: format!("sha256:{}", hex::encode(inventory.hasher.finalize())),
            by_path: BTreeMap::new(),
            by_span: BTreeMap::new(),
            starts: BTreeMap::new(),
            ends: BTreeMap::new(),
        };
        for (path, span) in inventory.spans {
            syntax.by_span.entry(span).or_insert_with(|| path.clone());
            syntax
                .starts
                .entry((span.start, span.expansion))
                .or_insert_with(|| path.clone());
            syntax
                .ends
                .entry((span.end, span.expansion))
                .or_insert_with(|| path.clone());
            syntax.by_path.insert(path, span);
        }
        Ok(syntax)
    }
}

// ---- Syntax inventory serializer ----

/// Serialization failure of an owner's syntax.
#[derive(Debug)]
struct InventoryError(String);

impl Display for InventoryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "syntax inventory serialization failed: {}", self.0)
    }
}

impl std::error::Error for InventoryError {}

impl ser::Error for InventoryError {
    fn custom<T: Display>(message: T) -> Self {
        Self(message.to_string())
    }
}

/// One pass over an owner's syntax: every `HirSourceSpan` is recorded with its structural path and contributes only
/// its expansion identity to the hash; every other value is hashed with a type tag and explicit lengths.
#[derive(Default)]
struct SyntaxInventory {
    hasher: Sha256,
    path: Vec<String>,
    /// Next element index of each open sequence, tuple or map.
    indices: Vec<usize>,
    spans: Vec<(String, Span)>,
    /// The source range being read, while its fields are serialized.
    span: Option<SpanFields>,
}

/// Fields of the source range currently being read.
#[derive(Default)]
struct SpanFields {
    field: &'static str,
    start: usize,
    end: usize,
    expansion: Option<Vec<u8>>,
}

type InventoryResult = Result<(), InventoryError>;

impl SyntaxInventory {
    /// Hash a tagged, length-delimited value.
    fn scalar(&mut self, tag: u8, bytes: &[u8]) -> InventoryResult {
        self.hasher.update([tag]);
        self.hasher.update((bytes.len() as u64).to_le_bytes());
        self.hasher.update(bytes);
        Ok(())
    }

    /// Record an unsigned field of the range being read, or hash an ordinary unsigned value.
    fn unsigned(&mut self, tag: u8, value: u64) -> InventoryResult {
        let Some(span) = self.span.as_mut() else {
            return self.scalar(tag, &value.to_le_bytes());
        };
        let value = usize::try_from(value).map_err(|error| InventoryError(error.to_string()))?;
        match span.field {
            "start" => span.start = value,
            "end" => span.end = value,
            _ => span.expansion.get_or_insert_with(Vec::new).push(value as u8),
        }
        Ok(())
    }

    /// Enter a named role of the structural path and hash the role name.
    fn enter(&mut self, role: &str) {
        self.hasher.update((role.len() as u64).to_le_bytes());
        self.hasher.update(role.as_bytes());
        self.path.push(role.to_owned());
    }

    /// Serialize the next element of the innermost open sequence under its index role.
    fn element<T: ?Sized + Serialize>(&mut self, value: &T) -> InventoryResult {
        if self.span.is_some() {
            return value.serialize(&mut *self);
        }
        let index = self.indices.last_mut().map_or(0, |next| {
            let index = *next;
            *next += 1;
            index
        });
        self.path.push(index.to_string());
        value.serialize(&mut *self)?;
        self.path.pop();
        Ok(())
    }

    /// Open a sequence-like value with its length.
    fn open(&mut self, tag: u8, length: Option<usize>) {
        if self.span.is_none() {
            self.hasher.update([tag]);
            self.hasher.update((length.unwrap_or_default() as u64).to_le_bytes());
            self.indices.push(0);
        }
    }

    /// Close a sequence-like value.
    fn close(&mut self) -> InventoryResult {
        if self.span.is_none() {
            self.indices.pop();
        }
        Ok(())
    }

    /// Finish the range being read: record it under its path and hash only its expansion identity.
    fn finish_span(&mut self) -> InventoryResult {
        let Some(fields) = self.span.take() else {
            return Ok(());
        };
        let expansion = match fields.expansion {
            Some(bytes) => {
                let array: [u8; 32] = bytes
                    .try_into()
                    .map_err(|_| InventoryError("an expansion identity is not 32 bytes".to_owned()))?;
                self.hasher.update([0x40]);
                self.hasher.update(array);
                Some(array)
            }
            None => {
                self.hasher.update([0x41]);
                None
            }
        };
        let span = Span::new(fields.start, fields.end).with_expansion(expansion);
        self.spans.push((self.path.join("/"), span));
        Ok(())
    }
}

impl<'inventory> ser::Serializer for &'inventory mut SyntaxInventory {
    type Ok = ();
    type Error = InventoryError;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;

    /// Hash a boolean.
    fn serialize_bool(self, value: bool) -> InventoryResult {
        self.scalar(0x01, &[u8::from(value)])
    }

    /// Hash an 8-bit signed integer.
    fn serialize_i8(self, value: i8) -> InventoryResult {
        self.scalar(0x02, &value.to_le_bytes())
    }

    /// Hash a 16-bit signed integer.
    fn serialize_i16(self, value: i16) -> InventoryResult {
        self.scalar(0x03, &value.to_le_bytes())
    }

    /// Hash a 32-bit signed integer.
    fn serialize_i32(self, value: i32) -> InventoryResult {
        self.scalar(0x04, &value.to_le_bytes())
    }

    /// Hash a 64-bit signed integer.
    fn serialize_i64(self, value: i64) -> InventoryResult {
        self.scalar(0x05, &value.to_le_bytes())
    }

    /// Hash a 128-bit signed integer, which integer literals can carry.
    fn serialize_i128(self, value: i128) -> InventoryResult {
        self.scalar(0x06, &value.to_le_bytes())
    }

    /// Hash an 8-bit unsigned integer, or read one expansion-identity byte of the current range.
    fn serialize_u8(self, value: u8) -> InventoryResult {
        self.unsigned(0x07, u64::from(value))
    }

    /// Hash a 16-bit unsigned integer.
    fn serialize_u16(self, value: u16) -> InventoryResult {
        self.unsigned(0x08, u64::from(value))
    }

    /// Hash a 32-bit unsigned integer.
    fn serialize_u32(self, value: u32) -> InventoryResult {
        self.unsigned(0x09, u64::from(value))
    }

    /// Hash a 64-bit unsigned integer, or read a coordinate of the current range without hashing it.
    fn serialize_u64(self, value: u64) -> InventoryResult {
        self.unsigned(0x0a, value)
    }

    /// Hash a 128-bit unsigned integer, which integer-literal magnitudes carry.
    fn serialize_u128(self, value: u128) -> InventoryResult {
        self.scalar(0x0b, &value.to_le_bytes())
    }

    /// Hash a 32-bit float by its bits.
    fn serialize_f32(self, value: f32) -> InventoryResult {
        self.scalar(0x0c, &value.to_le_bytes())
    }

    /// Hash a 64-bit float by its bits.
    fn serialize_f64(self, value: f64) -> InventoryResult {
        self.scalar(0x0d, &value.to_le_bytes())
    }

    /// Hash a character by its UTF-8 encoding.
    fn serialize_char(self, value: char) -> InventoryResult {
        self.scalar(0x0e, value.to_string().as_bytes())
    }

    /// Hash a string.
    fn serialize_str(self, value: &str) -> InventoryResult {
        self.scalar(0x0f, value.as_bytes())
    }

    /// Hash a byte string.
    fn serialize_bytes(self, value: &[u8]) -> InventoryResult {
        self.scalar(0x10, value)
    }

    /// Hash an absent value; an absent expansion identity is recorded when its range finishes.
    fn serialize_none(self) -> InventoryResult {
        if self.span.is_some() {
            return Ok(());
        }
        self.scalar(0x11, &[])
    }

    /// Hash a present value, or read a present expansion identity of the current range.
    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> InventoryResult {
        if self.span.is_none() {
            self.scalar(0x12, &[])?;
        }
        value.serialize(self)
    }

    /// Hash a unit value.
    fn serialize_unit(self) -> InventoryResult {
        self.scalar(0x13, &[])
    }

    /// Hash a unit struct by name.
    fn serialize_unit_struct(self, name: &'static str) -> InventoryResult {
        self.scalar(0x14, name.as_bytes())
    }

    /// Hash a payload-free variant by name.
    fn serialize_unit_variant(self, _name: &'static str, _index: u32, variant: &'static str) -> InventoryResult {
        self.scalar(0x15, variant.as_bytes())
    }

    /// Serialize a newtype's single value in place.
    fn serialize_newtype_struct<T: ?Sized + Serialize>(self, _name: &'static str, value: &T) -> InventoryResult {
        value.serialize(self)
    }

    /// Serialize a single-payload variant under its variant role.
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        value: &T,
    ) -> InventoryResult {
        self.hasher.update([0x16]);
        self.enter(variant);
        value.serialize(&mut *self)?;
        self.path.pop();
        Ok(())
    }

    /// Open a sequence whose elements take index roles.
    fn serialize_seq(self, length: Option<usize>) -> Result<Self, InventoryError> {
        self.open(0x17, length);
        Ok(self)
    }

    /// Open a tuple whose elements take index roles; an expansion identity's bytes are read instead.
    fn serialize_tuple(self, length: usize) -> Result<Self, InventoryError> {
        self.open(0x18, Some(length));
        Ok(self)
    }

    /// Open a tuple struct whose fields take index roles.
    fn serialize_tuple_struct(self, _name: &'static str, length: usize) -> Result<Self, InventoryError> {
        self.open(0x19, Some(length));
        Ok(self)
    }

    /// Open a tuple variant under its variant role; its fields take index roles.
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        length: usize,
    ) -> Result<Self, InventoryError> {
        self.hasher.update([0x1a]);
        self.enter(variant);
        self.open(0x1b, Some(length));
        Ok(self)
    }

    /// Open a map whose keys and values take alternating index roles.
    fn serialize_map(self, length: Option<usize>) -> Result<Self, InventoryError> {
        self.open(0x1c, length);
        Ok(self)
    }

    /// Open a struct, or begin reading a source range when the struct is `HirSourceSpan`.
    fn serialize_struct(self, name: &'static str, _length: usize) -> Result<Self, InventoryError> {
        if name == "HirSourceSpan" {
            self.span = Some(SpanFields::default());
        } else {
            self.scalar(0x1d, name.as_bytes())?;
        }
        Ok(self)
    }

    /// Open a struct variant under its variant role; its fields take their field-name roles.
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        _length: usize,
    ) -> Result<Self, InventoryError> {
        self.hasher.update([0x1e]);
        self.enter(variant);
        Ok(self)
    }
}

impl ser::SerializeSeq for &mut SyntaxInventory {
    type Ok = ();
    type Error = InventoryError;

    /// Serialize the next element under its index role.
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> InventoryResult {
        self.element(value)
    }

    /// Close the value and the roles it opened.
    fn end(self) -> InventoryResult {
        self.close()
    }
}

impl ser::SerializeTuple for &mut SyntaxInventory {
    type Ok = ();
    type Error = InventoryError;

    /// Serialize the next element under its index role.
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> InventoryResult {
        self.element(value)
    }

    /// Close the value and the roles it opened.
    fn end(self) -> InventoryResult {
        self.close()
    }
}

impl ser::SerializeTupleStruct for &mut SyntaxInventory {
    type Ok = ();
    type Error = InventoryError;

    /// Serialize the next positional field under its index role.
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> InventoryResult {
        self.element(value)
    }

    /// Close the value and the roles it opened.
    fn end(self) -> InventoryResult {
        self.close()
    }
}

impl ser::SerializeTupleVariant for &mut SyntaxInventory {
    type Ok = ();
    type Error = InventoryError;

    /// Serialize the next positional field under its index role.
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> InventoryResult {
        self.element(value)
    }

    /// Close the value and the roles it opened.
    fn end(self) -> InventoryResult {
        self.close()?;
        self.path.pop();
        Ok(())
    }
}

impl ser::SerializeMap for &mut SyntaxInventory {
    type Ok = ();
    type Error = InventoryError;

    /// Serialize a map key under its index role.
    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> InventoryResult {
        self.element(key)
    }

    /// Serialize a map value under its index role.
    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> InventoryResult {
        self.element(value)
    }

    /// Close the value and the roles it opened.
    fn end(self) -> InventoryResult {
        self.close()
    }
}

impl ser::SerializeStruct for &mut SyntaxInventory {
    type Ok = ();
    type Error = InventoryError;

    /// Serialize a named field under its field-name role, or read a field of the current source range.
    fn serialize_field<T: ?Sized + Serialize>(&mut self, key: &'static str, value: &T) -> InventoryResult {
        if let Some(span) = self.span.as_mut() {
            span.field = key;
            return value.serialize(&mut **self);
        }
        self.enter(key);
        value.serialize(&mut **self)?;
        self.path.pop();
        Ok(())
    }

    /// Close the struct; a finished source range is recorded under its structural path.
    fn end(self) -> InventoryResult {
        self.finish_span()
    }
}

impl ser::SerializeStructVariant for &mut SyntaxInventory {
    type Ok = ();
    type Error = InventoryError;

    /// Serialize a named field under its field-name role, or read a field of the current source range.
    fn serialize_field<T: ?Sized + Serialize>(&mut self, key: &'static str, value: &T) -> InventoryResult {
        self.enter(key);
        value.serialize(&mut **self)?;
        self.path.pop();
        Ok(())
    }

    /// Close the value and the roles it opened.
    fn end(self) -> InventoryResult {
        self.path.pop();
        Ok(())
    }
}

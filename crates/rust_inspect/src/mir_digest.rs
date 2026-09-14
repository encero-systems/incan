//! Digesting a Rust function body from lowered MIR.
//!
//! The token-stream digest in [`crate::digest`] is sound and deliberately coarse: any token change in an item moves
//! it, so it over-invalidates. This is the tighter answer for module-level functions, taken from what the compiler
//! lowered rather than from what the source spelled.
//!
//! # Why this reconstructs an id
//!
//! `ra_ap_hir_ty` exposes `HirDatabase::mir_body`, which takes a `hir_def::DefWithBodyId`. `ra_ap_hir` — the crate
//! whose job is to be the public API — has no way to hand one out: `DefWithBody::id()` is private, `Function`'s
//! inner id is `pub(crate)`, and the `hir_def` ids reach it through a plain `use` that re-exports only `ModuleId`.
//!
//! So the id is rebuilt from parts that *are* public: an `AstId` from `ExpandDatabase::ast_id_map`, a container
//! from the publicly re-exported `ModuleId`, and `InternDatabase::intern_function` to turn the resulting
//! `AssocItemLoc` into a `FunctionId`. Nothing here re-implements name resolution; it assembles a value out of
//! published pieces that the convenience conversion happens to keep private.
//!
//! **Why the rebuilt id is the same id.** Interning is deduplicating by definition: interning a `FunctionLoc` equal
//! to one rust-analyzer already interned returns that same `FunctionId`. Equality of the loc is equality of its two
//! public fields, so matching the container and the ast id is sufficient.
//!
//! **Why that argument has a precondition.** It holds only when the container this module can build is the
//! container rust-analyzer used. The one container reachable from the public API is `ItemContainerId::ModuleId`, so
//! it holds for module-level functions and fails for anything owned by an `impl` or a `trait`. Interning a loc
//! rust-analyzer never interned does not fail loudly — it mints a *new* id and lowers a body for it — so an
//! associated function is rejected up front rather than allowed to produce a plausible digest of the wrong thing.
//! That rejection is a test, not a comment.
//!
//! # Why not `debug_mir`
//!
//! `DefWithBody::debug_mir` is public and body-level, so the obvious question is why this module does not simply
//! hash it. Both it and the fields hashed below are rendered through formatting traits, so "rendered text" is not
//! by itself the distinction. Two other things are.
//!
//! It renders the *function*, not the body: its first line is `fn <name>(…)`, so two functions with identical
//! bodies render differently. A digest built on it would answer "is this the same function's body", which is the
//! question the declaration identity already answers, instead of "is this the same body". The tests below confirm
//! this by measurement and work around it by dropping that header line.
//!
//! And the digest must be a function of the body alone. `debug_mir` renders for a human, and its own documentation
//! calls it "for debugging purposes" — the set of fields it prints is not a contract. The fields hashed below are
//! chosen one at a time, and each choice is checkable.
//!
//! With the header line removed, `debug_mir` is a genuine *test oracle*: an independent second opinion, computed by
//! code this module does not own, on whether two bodies really differ. It is used that way below and never as a
//! digest input.
//!
//! # What is excluded, and why the digest is still not span-free
//!
//! `Statement` and `Terminator` each carry a `MirSpan` beside their `kind`, and only the kind is hashed. That
//! exclusion is not what makes the digest position-independent, and saying so would be wrong: `StatementKind`
//! embeds spans of its own — an `Operand` carries `span: Some(ExprId(..))` — so spans do reach the hash.
//!
//! What holds instead is narrower and is what actually matters. Every span reaching the hash is a body-local arena
//! index, never a file offset. Moving a function down its file, or adding comments and line breaks inside it, does
//! not renumber that body's expressions, so the digest does not move. Adding the excluded `MirSpan` back is not
//! caught by the suite below, for exactly this reason: equivalent information already flows in through the kinds.
//! The field is excluded because a position is not lowered meaning, and because the exclusion is what keeps the
//! property above true if `MirSpan` ever gains a file offset.
//!
//! # What stability is claimed
//!
//! The digest is stable across processes and across positions in a file, for one pinned `ra_ap_*` version. It is
//! not stable across an `ra_ap_*` upgrade, because the `Debug` shapes it hashes may change. That is the safe
//! direction: an upgrade invalidates every body digest and rebuilds, which costs time and cannot ship a stale
//! artifact.

use ra_ap_hir::{AsAssocItem, Function, HasSource, attach_db};
use ra_ap_hir_def::db::InternDatabase;
use ra_ap_hir_def::{AssocItemLoc, DefWithBodyId, ItemContainerId};
use ra_ap_hir_expand::db::ExpandDatabase;
use ra_ap_hir_ty::db::HirDatabase;
use ra_ap_ide_db::RootDatabase;
use ra_ap_syntax::ast;
use sha2::{Digest, Sha256};

use crate::digest_tokens::absorb_field;

// ============================================================================
// Encoding tags
// ============================================================================
//
// Every value hashed below is introduced by a one-byte kind tag, so a run of locals cannot encode the same bytes as
// a run of statements. The tags are local to this digest; they share the length-delimiting primitive with the token
// digest but not its tag space.

/// Tag introducing the digest's own version marker.
const TAG_VERSION: u8 = b'V';
/// Tag introducing one local's declared type.
const TAG_LOCAL_TYPE: u8 = b'T';
/// Tag opening one basic block, so two bodies cannot differ only in where a block boundary falls.
const TAG_BLOCK: u8 = b'B';
/// Tag introducing one statement's kind.
const TAG_STATEMENT: u8 = b'S';
/// Tag introducing one block's terminator kind.
const TAG_TERMINATOR: u8 = b'X';
/// Tag standing in for a block that has no terminator, which is distinct from a block terminated by nothing.
const TAG_UNTERMINATED: u8 = b'U';
/// Tag introducing the parameter count.
const TAG_PARAM_COUNT: u8 = b'#';

/// Why a body could not be digested.
///
/// The variants are kept apart because callers must treat them differently. A declaration without a body is
/// ordinary. A body that failed to lower is not: a caller must treat it as *changed* rather than as unchanged,
/// because a body whose digest could not be computed is not a body known to be stable. An associated function is
/// neither — it is a limit of the reconstruction, and the caller should fall back to the token digest.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MirDigestError {
    /// The item has no body to lower, or its source could not be located.
    #[error("the function has no body to lower")]
    NoBody,
    /// MIR lowering was attempted and failed.
    #[error("MIR lowering failed for the function body")]
    LoweringFailed,
    /// The function is owned by an `impl` or a `trait` rather than by a module.
    ///
    /// Only `ItemContainerId::ModuleId` is reachable from the public API, so the id this module would rebuild for an
    /// associated function is not the id rust-analyzer interned for it. Refusing is the whole point: interning the
    /// wrong loc succeeds and yields a body, which is the one outcome a cache key must never be built on.
    #[error("the function is an associated item, whose lowered body this route cannot address")]
    NotAModuleLevelFunction,
}

/// Digest one module-level function's body from its lowered MIR.
///
/// The digest covers the lowered shape — declared locals, each block's statement kinds, and each block's terminator
/// — and excludes every source position. Two bodies that lower identically digest identically, however differently
/// they were written and wherever they sit in the file.
///
/// The owning module is read from the function rather than taken as an argument. A caller cannot then pass a module
/// the function does not belong to, which would intern an id rust-analyzer never created.
///
/// # Errors
///
/// [`MirDigestError::NotAModuleLevelFunction`] when the function belongs to an `impl` or a `trait`,
/// [`MirDigestError::NoBody`] when its source cannot be resolved, and [`MirDigestError::LoweringFailed`] when
/// lowering failed.
pub fn function_body_digest(db: &RootDatabase, function: Function) -> Result<String, MirDigestError> {
    attach_db(db, || {
        if function.as_assoc_item(db).is_some() {
            return Err(MirDigestError::NotAModuleLevelFunction);
        }

        let source = function.source(db).ok_or(MirDigestError::NoBody)?;
        let ast_id_map = db.ast_id_map(source.file_id);
        let file_ast_id = ast_id_map.ast_id(&source.value);

        let loc: AssocItemLoc<ast::Fn> = AssocItemLoc {
            container: ItemContainerId::ModuleId(function.module(db).into()),
            id: source.with_value(file_ast_id),
        };
        let id = DefWithBodyId::FunctionId(db.intern_function(loc));
        let body = db.mir_body(id).map_err(|_| MirDigestError::LoweringFailed)?;

        let mut hasher = Sha256::new();
        absorb_field(&mut hasher, TAG_VERSION, b"mir-v1");

        // Locals contribute their declared types in declaration order. An arena index is not hashed as a field of
        // its own: the index *is* the position in this run, so hashing the run in order already encodes it.
        for (_, local) in body.locals.iter() {
            absorb_field(&mut hasher, TAG_LOCAL_TYPE, format!("{:?}", local.ty).as_bytes());
        }

        // Blocks contribute statement kinds and terminator kinds. `Statement` carries a `MirSpan` beside its
        // `kind`, and that span moves whenever a line above it does, so only the kind is hashed.
        for (_, block) in body.basic_blocks.iter() {
            // Also redundant today: every block below emits exactly one terminator field, which already separates
            // one block's statements from the next. The tag is what keeps the grouping unambiguous if that ever
            // stops being true.
            absorb_field(&mut hasher, TAG_BLOCK, b"");
            for statement in &block.statements {
                absorb_field(&mut hasher, TAG_STATEMENT, format!("{:?}", statement.kind).as_bytes());
            }
            match &block.terminator {
                Some(terminator) => {
                    absorb_field(&mut hasher, TAG_TERMINATOR, format!("{:?}", terminator.kind).as_bytes())
                }
                None => absorb_field(&mut hasher, TAG_UNTERMINATED, b""),
            }
        }

        // Redundant against the locals above — parameters are the leading locals and their types are already
        // hashed in order — and no mutation of this suite kills its removal. It is retained because the redundancy
        // is a property of today's lowering, not a guarantee, and an extra field in a digest can only ever
        // over-invalidate.
        absorb_field(
            &mut hasher,
            TAG_PARAM_COUNT,
            body.param_locals.len().to_string().as_bytes(),
        );
        Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
    })
}

/// Tests for the MIR body digest.
///
/// The fixture is built around one test — [`tests::the_digest_partitions_functions_exactly_as_their_bodies_do`] —
/// and every function in it exists to make some way of computing the digest wrong. Mutating the digest and checking
/// that the suite notices is how that was established; the record, so the next reader does not have to redo it:
///
/// | Mutation | Caught by |
/// | --- | --- |
/// | Hash local types without length-delimiting them | `tagged_left` / `tagged_right` |
/// | Keep the tag byte but drop the length | `tagged_left` / `tagged_right` |
/// | Stop hashing locals | `widths_unsigned` |
/// | Stop hashing statements | `alpha` / `gamma` |
/// | Stop hashing terminators | `calls_one` / `calls_two` |
/// | Hash only the first basic block | `picks_larger` / `picks_smaller` |
/// | Accept an associated function instead of refusing it | the refusal test |
/// | Return a constant | any pair |
///
/// Three mutations are *not* caught, each because the field it removes is redundant rather than because the suite
/// is weak. Hashing `Statement`'s own span changes nothing, because operand spans already reach the hash through
/// the kinds. Dropping the parameter count changes nothing, because parameters are the leading locals and their
/// types are hashed in order. Dropping the per-block tag changes nothing, because every block emits exactly one
/// terminator field, which already separates one block's statements from the next. All three are retained: an extra
/// field in a digest can only over-invalidate, and each redundancy is a property of today's lowering rather than a
/// guarantee.
#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;

    use ra_ap_hir::{Crate, DefWithBody, Impl, ModuleDef, ScopeDef};
    use tempfile::TempDir;

    use super::{MirDigestError, function_body_digest};
    use crate::loader::RustWorkspace;

    /// Six functions forming four equivalence classes, plus one associated function.
    ///
    /// `alpha` and `beta` are the same body written differently and placed differently; `gamma` and `delta` are a
    /// second, genuinely different body. The doc comment, the line comment, and the odd line breaks all exist to be
    /// ignored, and `Holder::method` exists to be refused.
    ///
    /// `calls_one` and `calls_two` differ in nothing a statement can see. A call is a MIR *terminator*, so these
    /// two bodies have identical statement runs and differ only in the callee named by their terminator. Without
    /// them the suite cannot tell a digest that hashes terminators from one that ignores them.
    const PROBE_SOURCE: &str = r#"pub fn alpha(a: i32, b: i32) -> i32 { a + b }

/// A doc comment that must not reach the digest.
pub fn beta(a: i32, b: i32) -> i32 {
    // Written across several lines, and sitting further down the file.
    a
        +
        b
}

pub fn gamma(a: i32, b: i32) -> i32 { a - b }

pub fn delta(a: i32, b: i32) -> i32 { a - b }

pub fn calls_one(a: i32) -> i32 { helper_one(a) }

pub fn calls_two(a: i32) -> i32 { helper_two(a) }

fn helper_one(a: i32) -> i32 { a }

fn helper_two(a: i32) -> i32 { a + 1 }

pub fn widths_unsigned(a: u32, b: u32) -> u32 { a + b }

pub fn picks_larger(a: i32, b: i32) -> i32 { if a > b { a } else { b } }

pub fn picks_smaller(a: i32, b: i32) -> i32 { if a > b { b } else { a } }

pub struct T;
pub struct TT;
pub struct C;
pub struct TC;

pub fn tagged_left(x: TT, y: C) -> i32 { 0 }

pub fn tagged_right(x: T, y: TC) -> i32 { 0 }

pub struct Holder;

impl Holder {
    pub fn method(a: i32, b: i32) -> i32 { a + b }
}
"#;

    /// Build and load a single-crate workspace from one source file.
    ///
    /// The [`TempDir`] is returned alongside the workspace because dropping it deletes the sources the database
    /// still reads through.
    fn probe(source: &str) -> Result<(TempDir, RustWorkspace), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        fs::create_dir_all(tmp.path().join("src"))?;
        fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname = \"mir_digest_probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        fs::write(tmp.path().join("src/lib.rs"), source)?;
        let workspace = RustWorkspace::load(tmp.path(), &|_| ())?;
        Ok((tmp, workspace))
    }

    /// Resolve the probe crate, failing the test rather than skipping when the fixture did not load.
    fn probe_crate(workspace: &RustWorkspace) -> Result<Crate, Box<dyn std::error::Error>> {
        workspace
            .crate_by_name("mir_digest_probe")
            .ok_or_else(|| Box::<dyn std::error::Error>::from("the probe crate did not load"))
    }

    /// Find one module-level function by name in the probe crate's root module.
    fn free_function(workspace: &RustWorkspace, name: &str) -> Result<ra_ap_hir::Function, Box<dyn std::error::Error>> {
        let db = workspace.db();
        probe_crate(workspace)?
            .root_module(db)
            .scope(db, None)
            .into_iter()
            .find_map(|(scope_name, definition)| match definition {
                ScopeDef::ModuleDef(ModuleDef::Function(function)) if scope_name.as_str() == name => Some(function),
                _ => None,
            })
            .ok_or_else(|| Box::<dyn std::error::Error>::from(format!("probe crate has no free function `{name}`")))
    }

    /// Digest one named free function of the probe crate.
    fn digest_of(workspace: &RustWorkspace, name: &str) -> Result<String, Box<dyn std::error::Error>> {
        Ok(function_body_digest(workspace.db(), free_function(workspace, name)?)?)
    }

    /// Render one named free function's lowered body through rust-analyzer's debugging formatter.
    ///
    /// This is the oracle: an independent answer to "do these two bodies actually differ", computed by code this
    /// module does not own and never feeds into a digest.
    ///
    /// The rendering's first line is `fn <name>(…)`, which would make every function's body unique by construction
    /// and turn the oracle into a tautology. It is dropped, and nothing else is. Everything below that line — local
    /// declarations, statements, and terminators including the `span` field the digest omits — is compared as-is,
    /// so the oracle stays stricter than the digest rather than being tuned to agree with it.
    fn oracle_of(workspace: &RustWorkspace, name: &str) -> Result<String, Box<dyn std::error::Error>> {
        let db = workspace.db();
        let body = DefWithBody::from(free_function(workspace, name)?);
        let rendered = ra_ap_hir::attach_db(db, || body.debug_mir(db));
        let (header, rest) = rendered
            .split_once('\n')
            .ok_or("the oracle rendered a single line, so its signature header cannot be separated")?;
        assert!(
            header.starts_with("fn "),
            "the oracle's first line is no longer the signature header, so dropping it is no longer sound: {header}"
        );
        Ok(rest.to_owned())
    }

    /// Group names by the value a probe returns for them, so two groupings can be compared as partitions.
    ///
    /// The grouping keys are digests and renderings, which differ between the two probes being compared, so the
    /// classes are sorted by their own contents rather than left in key order. What is being compared is which
    /// names fall together, never what they fall together under.
    fn partition_by<F>(names: &[&str], mut value: F) -> Result<Vec<Vec<String>>, Box<dyn std::error::Error>>
    where
        F: FnMut(&str) -> Result<String, Box<dyn std::error::Error>>,
    {
        let mut classes: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for name in names {
            classes.entry(value(name)?).or_default().push((*name).to_owned());
        }
        let mut partition: Vec<Vec<String>> = classes.into_values().collect();
        for class in &mut partition {
            class.sort();
        }
        partition.sort();
        Ok(partition)
    }

    #[test]
    fn formatting_and_position_do_not_move_the_digest() -> Result<(), Box<dyn std::error::Error>> {
        let (_tmp, workspace) = probe(PROBE_SOURCE)?;
        assert_eq!(
            digest_of(&workspace, "alpha")?,
            digest_of(&workspace, "beta")?,
            "one body written on one line and again across four, further down the file, must digest the same"
        );
        Ok(())
    }

    #[test]
    fn a_changed_operator_moves_the_digest() -> Result<(), Box<dyn std::error::Error>> {
        let (_tmp, workspace) = probe(PROBE_SOURCE)?;
        assert_ne!(
            oracle_of(&workspace, "alpha")?,
            oracle_of(&workspace, "gamma")?,
            "the oracle must agree the two bodies differ, or this test proves nothing about the digest"
        );
        assert_ne!(digest_of(&workspace, "alpha")?, digest_of(&workspace, "gamma")?);
        Ok(())
    }

    #[test]
    fn the_digest_partitions_functions_exactly_as_their_bodies_do() -> Result<(), Box<dyn std::error::Error>> {
        let (_tmp, workspace) = probe(PROBE_SOURCE)?;
        let names = [
            "alpha",
            "beta",
            "gamma",
            "delta",
            "calls_one",
            "calls_two",
            "widths_unsigned",
            "picks_larger",
            "picks_smaller",
            "tagged_left",
            "tagged_right",
        ];

        // The reconstructed id is only correct if each function's digest comes from *that* function's body. A
        // bridge that resolved every name to one body, or that shifted names onto neighbouring bodies, would still
        // produce four digests; it would not reproduce the oracle's grouping.
        assert_eq!(
            partition_by(&names, |name| digest_of(&workspace, name))?,
            partition_by(&names, |name| oracle_of(&workspace, name))?,
            "digest equality must hold exactly where lowered-body equality holds"
        );
        Ok(())
    }

    #[test]
    fn an_associated_function_is_refused_rather_than_digested() -> Result<(), Box<dyn std::error::Error>> {
        let (_tmp, workspace) = probe(PROBE_SOURCE)?;
        let db = workspace.db();
        let method = Impl::all_in_crate(db, probe_crate(&workspace)?)
            .into_iter()
            .flat_map(|block| block.items(db))
            .find_map(|item| match item {
                ra_ap_hir::AssocItem::Function(function) => Some(function),
                _ => None,
            })
            .ok_or("probe crate has no associated function")?;

        // Interning a loc rust-analyzer never interned succeeds and yields a body, so the failure this guards
        // against is a plausible digest of the wrong thing rather than a crash.
        assert_eq!(
            function_body_digest(db, method),
            Err(MirDigestError::NotAModuleLevelFunction)
        );
        Ok(())
    }

    #[test]
    fn the_digest_survives_a_second_database() -> Result<(), Box<dyn std::error::Error>> {
        let (_first_tmp, first) = probe(PROBE_SOURCE)?;
        let shifted = format!("pub fn interned_first(a: i32) -> i32 {{ a }}\n\n{PROBE_SOURCE}");
        let (_second_tmp, second) = probe(&shifted)?;

        // A different path, a different file, and a different interning order. If any salsa-assigned id reached the
        // hash through a `Debug` rendering, these would differ and the digest would be useless across processes.
        assert_eq!(digest_of(&first, "alpha")?, digest_of(&second, "alpha")?);
        Ok(())
    }
}

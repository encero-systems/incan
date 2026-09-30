//! A module names a type another module declares without importing it, through the field of a model or the parameter
//! of a function it imports, and the program builds (#1561): an empty list, dict or set, a `None`, and a nested or
//! tuple-typed empty collection passed where that type is expected are spelled by the type's declaring module.

use super::generated_programs::run_modules_with_stdlib;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The module that declares the types the importing module never imports: a newtype, a model and an enum.
const IDS: &str = r#"
@derive(Eq, Hash)
pub type EvidenceId = newtype str


pub model Evidence:
    pub id: str


pub enum Kind:
    Primary
    Secondary
"#;

/// The module that imports those types and declares a model whose fields, and functions whose parameters, name them.
const RECORDS: &str = r#"
from ids import EvidenceId, Evidence, Kind


pub model Decision:
    pub name: str
    pub admitted_evidence_ids: list[EvidenceId]
    pub scores: dict[EvidenceId, int]
    pub seen: set[EvidenceId]
    pub first: Option[EvidenceId]
    pub nested: list[list[Evidence]]
    pub pairs: list[tuple[EvidenceId, Kind]]
    pub kinds: dict[str, Kind]


pub def make_id(text: str) -> EvidenceId:
    return EvidenceId(text)


pub def count_kinds(kinds: list[Kind]) -> int:
    return len(kinds)


pub def describe(first: Option[Evidence]) -> str:
    match first:
        Some(evidence) => return evidence.id
        None => return "none"
"#;

/// `Decision(admitted_evidence_ids=[], ...)` in a module that imports `Decision` but none of `EvidenceId`, `Evidence`
/// and `Kind` builds its empty list, dict and set, its `None`, its nested and tuple-typed empty lists, and the empty
/// list and `None` it passes to an imported function, with the declaring module's types.
#[test]
fn empty_collections_and_none_for_types_the_module_does_not_import_build_issue1561() -> TestResult {
    let stdout = run_modules_with_stdlib(
        &[("ids", IDS), ("records", RECORDS)],
        r#"
from records import Decision, make_id, count_kinds, describe


def main() -> None:
    empty = Decision(name="a", admitted_evidence_ids=[], scores={}, seen=set(), first=None, nested=[[]], pairs=[], kinds={})
    println(len(empty.admitted_evidence_ids) + len(empty.scores) + len(empty.seen) + len(empty.pairs) + len(empty.kinds))
    println(empty.first is None)
    println(len(empty.nested))
    println(empty.admitted_evidence_ids == [])
    full = Decision(name="b", admitted_evidence_ids=[make_id("e1")], scores={make_id("e1"): 3}, seen=set(), first=Some(make_id("e2")), nested=[], pairs=[], kinds={})
    println(len(full.admitted_evidence_ids))
    println(count_kinds([]))
    println(describe(None))
"#,
    )?;
    assert_eq!(stdout, "0\ntrue\n1\ntrue\n1\n0\nnone\n");
    Ok(())
}

/// A module that imports the type itself, under its own name or another, keeps spelling it through that import, beside
/// an empty list of a type it does not import.
#[test]
fn a_type_the_module_imports_keeps_its_import_beside_an_unimported_one_issue1561() -> TestResult {
    let stdout = run_modules_with_stdlib(
        &[("ids", IDS), ("records", RECORDS)],
        r#"
from ids import Kind
from ids import Evidence as Proof
from records import Decision, count_kinds


def main() -> None:
    decision = Decision(name="a", admitted_evidence_ids=[], scores={}, seen=set(), first=None, nested=[], pairs=[], kinds={})
    mut chosen: list[Kind] = []
    chosen.append(Kind.Primary)
    mut proofs: list[Proof] = []
    proofs.append(Proof(id="p"))
    println(count_kinds(chosen) + len(decision.admitted_evidence_ids))
    println(proofs[0].id)
"#,
    )?;
    assert_eq!(stdout, "1\np\n");
    Ok(())
}

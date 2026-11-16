//! RFC 164: an explicit view input reaches an ordinary parameter shared; the call is refused once the callee's
//! realization would consume it, since filling it would copy borrowed storage.

use super::*;

/// A callee that only reads an explicit shared input receives it shared; one that stores it would need a silent
/// clone, so the call is refused in Body IR after the shared-read fixed point.
#[test]
fn explicit_view_inputs_reach_ordinary_parameters_only_shared() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
model Page:
    title: str

def show(page: Page) -> int:
    return len(page.title)

def keep(page: Page, mut pages: list[Page]) -> None:
    pages.append(page)

def reads(page: Borrowed[Page]) -> int:
    return show(page)

def stores(page: Borrowed[Page], mut pages: BorrowedMut[list[Page]]) -> None:
    keep(page, pages)
"#;
    let (_, _, mut module) = checked_seed(source)?;
    super::shared_reads::retain_modules(
        std::slice::from_mut(&mut module),
        crate::body_ir::ContextModules::default(),
    );
    let body = |name: &str| {
        module
            .bodies
            .iter()
            .find(|body| body.name == name)
            .ok_or(format!("missing body {name}"))
    };
    assert!(
        body("reads")?.first_representation_gap().is_none(),
        "{:#?}",
        body("reads")?
    );
    let gap = body("stores")?.first_representation_gap();
    assert!(
        gap.is_some_and(|gap| gap.contains("needs an explicit clone")),
        "{gap:?}"
    );
    Ok(())
}

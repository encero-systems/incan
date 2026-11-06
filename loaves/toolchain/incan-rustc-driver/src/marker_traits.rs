//! Empty native standard trait declarations authorized by checked Incan lowering.

use crate::ast_build::{global_path, ident, inherited, path_ty};
use crate::error::PlanError;
use crate::plan::{MarkerTrait, Plan};
use rustc_ast as ast;
use rustc_span::Span;
use thin_vec::ThinVec;

/// Require the explicitly supplied SDK crate before an authorized empty implementation reaches rustc.
pub(crate) fn native_root(plan: &Plan) -> Option<&'static str> {
    plan.models
        .iter()
        .any(|model| !model.marker_traits.is_empty())
        .then_some("incan_stdlib_data")
}

/// Map the closed authorization enum to its native item and required existing derive; source selection stays in Incan.
fn descriptor(marker: &MarkerTrait) -> (usize, &'static str, &'static str) {
    match marker {
        MarkerTrait::TomlSerialize => (0, "TomlSerialize", "serde::Serialize"),
        MarkerTrait::TomlDeserialize => (1, "TomlDeserialize", "serde::Deserialize"),
    }
}

/// Declare only complete empty marker contracts on source layouts, leaving every supertrait obligation to rustc.
pub(crate) fn items(plan: &Plan, span: Span) -> Result<Vec<Box<ast::Item>>, PlanError> {
    let mut result = Vec::new();
    for model in &plan.models {
        let mut selected = [false; 2];
        for marker in &model.marker_traits {
            let (slot, name, required_derive) = descriptor(marker);
            if !model.library_item.is_empty()
                || selected[slot]
                || !model.derives.iter().any(|derive| derive == required_derive)
            {
                return Err(PlanError::Invalid {
                    function: model.name.clone(),
                    reason: "unsupported native TOML marker trait contract".into(),
                });
            }
            selected[slot] = true;
            result.push(empty_impl(&model.name, name, span));
        }
    }
    Ok(result)
}

/// Build fixed-path trait syntax for the authorized source type; no user-provided trait path enters this boundary.
fn empty_impl(owner: &str, name: &str, span: Span) -> Box<ast::Item> {
    let path = global_path(&["incan_stdlib_data", "toml", name], span);
    let mut item = crate::declarations::item(
        ast::ItemKind::Impl(ast::Impl {
            generics: ast::Generics::default(),
            constness: ast::Const::No,
            of_trait: Some(Box::new(ast::TraitImplHeader {
                defaultness: ast::Defaultness::Implicit,
                safety: ast::Safety::Default,
                polarity: ast::ImplPolarity::Positive,
                trait_ref: ast::TraitRef {
                    path,
                    ref_id: ast::DUMMY_NODE_ID,
                },
            })),
            self_ty: path_ty(ast::Path::from_ident(ident(owner, span)), span),
            items: ThinVec::new(),
        }),
        span,
    );
    item.vis = inherited(span);
    item
}

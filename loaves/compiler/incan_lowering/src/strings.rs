//! Admission table for Body IR's canonical text operations.

use incan_lang::lang::surface::string_methods::StringMethodId;
use incan_mir_plan_native::{ListLeaf, Operand, Place, PlanType, SourceSpan};
use incan_semantics_core::body_ir::HelperOp;

use crate::frame::{Builder, external_call, temporary};
use crate::lists::is_list;

/// Select text-only helpers whose parameters borrow strings and whose runtime semantics match legacy.
pub fn text_helper(helper: HelperOp) -> Option<(&'static str, PlanType)> {
    let selected = match helper {
        HelperOp::StrConcat => ("concat_text", PlanType::String),
        HelperOp::StrEq => ("equal_text", PlanType::Bool),
        HelperOp::StrNe => ("unequal_text", PlanType::Bool),
        HelperOp::StrLt => ("less_text", PlanType::Bool),
        HelperOp::StrLe => ("less_equal_text", PlanType::Bool),
        HelperOp::StrGt => ("greater_text", PlanType::Bool),
        HelperOp::StrGe => ("greater_equal_text", PlanType::Bool),
        HelperOp::StrLen => ("length_text", PlanType::Int),
        HelperOp::StrUpper => ("upper_text", PlanType::String),
        HelperOp::StrLower => ("lower_text", PlanType::String),
        HelperOp::StrStrip => ("strip_text", PlanType::String),
        HelperOp::StrReplace => ("replace_text", PlanType::String),
        HelperOp::StrNotContains => ("not_contains_text", PlanType::Bool),
        HelperOp::StrContains => ("contains_text", PlanType::Bool),
        HelperOp::TextCodecEncodingValid => ("text_codec_encoding_valid", PlanType::Bool),
        HelperOp::TextCodecErrorsValid => ("text_codec_errors_valid", PlanType::Bool),
        _ => return None,
    };
    Some(selected)
}

/// Retain the positional runtime signatures for text-only helpers.
pub fn text_helper_arity(name: &str) -> i64 {
    const UNARY: [&str; 8] = [
        "length_text",
        "upper_text",
        "lower_text",
        "strip_text",
        "clone_text",
        "split_whitespace_text",
        "text_codec_encoding_valid",
        "text_codec_errors_valid",
    ];
    if UNARY.contains(&name) {
        return 1;
    }
    if name == "replace_text" {
        return 3;
    }
    2
}

/// Select a checker-authenticated string operation and its runtime helper.
pub fn text_method(method: StringMethodId, receiver: &PlanType) -> Option<(&'static str, PlanType)> {
    if !matches!(receiver, PlanType::String | PlanType::StringRef | PlanType::FrozenStr) {
        return None;
    }
    match method {
        StringMethodId::ToString => Some(("clone_text", PlanType::String)),
        StringMethodId::StartsWith => Some(("starts_text", PlanType::Bool)),
        StringMethodId::EndsWith => Some(("ends_text", PlanType::Bool)),
        StringMethodId::SplitWhitespace => Some(("split_whitespace_text", PlanType::List(ListLeaf::Str, 1))),
        _ => None,
    }
}

/// Emit one canonical borrowed-text call and retain its owned result for transfer or discard.
pub fn emit_text_helper(
    builder: &mut Builder,
    name: &str,
    result: PlanType,
    parameters: Vec<PlanType>,
    operands: Vec<Operand>,
    span: &SourceSpan,
) -> Place {
    let text = temporary(builder, result.clone(), span);
    let owned = is_list(&result);
    external_call(
        builder,
        &format!("incan_native_runtime::{name}"),
        parameters,
        result,
        operands,
        text.clone(),
        span,
    );
    if owned {
        builder.owned.push(text.local);
    }
    text
}

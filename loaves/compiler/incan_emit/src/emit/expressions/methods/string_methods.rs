use proc_macro2::TokenStream;
use quote::quote;

use crate::emit::expressions::methods::ReceiverInfo;
use crate::emit::{EmitError, IrEmitter};
use incan_ir::expr::{BytesMethodKind, IrCallArg, IrCallArgKind, IrExprKind, StringMethodKind, TypedExpr};
use incan_lang::lang::text_codecs::{self, DecodeErrorsPolicy};

/// The optional codec labels of a `str.encode` / `bytes.decode` call, bound by position or by name (#1668).
///
/// The typechecker already rejected every other argument shape, so this only has to put a named `errors=` back in
/// its slot: the ordinary known-method path drops argument names, which would otherwise read `data.decode(errors=
/// "replace")` as an encoding label.
#[derive(Default)]
pub struct TextCodecCallArgs<'a> {
    /// The `encoding` label, when supplied.
    pub encoding: Option<&'a TypedExpr>,
    /// The `errors` policy, when supplied (`bytes.decode` only).
    pub errors: Option<&'a TypedExpr>,
}

impl<'a> TextCodecCallArgs<'a> {
    /// Bind lowered call arguments to the `encoding` / `errors` slots.
    pub fn from_call_args(args: &'a [IrCallArg]) -> Self {
        let mut bound = Self::default();
        let mut positional = 0usize;
        for arg in args {
            let slot = match (&arg.kind, arg.name.as_deref()) {
                (IrCallArgKind::Named, Some("errors")) => &mut bound.errors,
                (IrCallArgKind::Named, _) => &mut bound.encoding,
                _ => {
                    positional += 1;
                    if positional == 1 {
                        &mut bound.encoding
                    } else {
                        &mut bound.errors
                    }
                }
            };
            *slot = Some(&arg.expr);
        }
        bound
    }
}

/// Return the literal text of a string-literal argument, if the label is known at compile time.
fn literal_label(expr: Option<&TypedExpr>) -> Option<&str> {
    match expr.map(|expr| &expr.kind) {
        Some(IrExprKind::String(text)) => Some(text.as_str()),
        _ => None,
    }
}

/// How a text codec call reports a label or input it cannot handle.
#[derive(Clone, Copy)]
enum CodecFailure {
    /// `str.encode`: an unknown run-time label is a programming error and raises `ValueError`, as `int("x")` does.
    Raise,
    /// `bytes.decode`: every failure is an `Err(ValidationError)` the caller handles (#1668).
    Err,
}

/// The `Result` type `bytes.decode` returns, spelled in full so each arm's `Ok`/`Err` infers without a binding.
fn decode_result_type() -> TokenStream {
    quote! { ::std::result::Result<String, incan_std_core::validation::ValidationError> }
}

/// Emit the `Err` arm of a decode failure: a `ValidationError` whose `code` names the failure category.
fn decode_failure(message: TokenStream, code: &str) -> TokenStream {
    let result_type = decode_result_type();
    quote! {
        <#result_type>::Err(incan_std_core::validation::ValidationError::with_code(#message, #code))
    }
}

/// Wrap `body` in the runtime UTF-8 label guard when the encoding is not a literal the typechecker already accepted.
///
/// The guard normalizes the label exactly like `text_codecs::normalize_encoding_label`; anything else is reported
/// the way `failure` says the callee reports it.
fn guard_runtime_encoding_label(
    emitter: &IrEmitter,
    callee: &str,
    encoding: Option<&TypedExpr>,
    failure: CodecFailure,
    body: TokenStream,
) -> Result<TokenStream, EmitError> {
    let Some(encoding) = encoding else {
        return Ok(body);
    };
    if literal_label(Some(encoding)).is_some() {
        // The typechecker admitted this literal, so it names UTF-8 and needs no runtime check.
        return Ok(body);
    }
    let label = emitter.emit_expr(encoding)?;
    let utf8_labels = text_codecs::UTF8_ENCODING_LABELS;
    let message = format!("{callee}() supports only utf-8 in this release, got encoding '{{__incan_encoding}}'");
    let unknown = match failure {
        CodecFailure::Raise => quote! { incan_std_core::errors::raise_value_error(&format!(#message)) },
        CodecFailure::Err => decode_failure(quote! { format!(#message) }, "unknown-encoding"),
    };
    Ok(quote! {
        match <_ as AsRef<str>>::as_ref(&#label)
            .trim()
            .to_ascii_lowercase()
            .replace('_', "-")
            .as_str()
        {
            #(#utf8_labels)|* => #body,
            __incan_encoding => #unknown,
        }
    })
}

/// Emit `text.encode(encoding="utf-8")` as an owned UTF-8 byte vector.
fn emit_str_encode(emitter: &IrEmitter, info: &ReceiverInfo, args: &[IrCallArg]) -> Result<TokenStream, EmitError> {
    let r = &info.r;
    let bound = TextCodecCallArgs::from_call_args(args);
    guard_runtime_encoding_label(
        emitter,
        "str.encode",
        bound.encoding,
        CodecFailure::Raise,
        quote! { (#r).as_bytes().to_vec() },
    )
}

/// Emit the strict UTF-8 decode of a byte view: valid input is `Ok` of an owned `String`, malformed input is `Err`
/// of a `ValidationError` coded `invalid-utf8` whose message names the offending offset.
fn strict_decode(view: &TokenStream) -> TokenStream {
    let result_type = decode_result_type();
    let failure = decode_failure(
        quote! { format!("'utf-8' codec can't decode bytes: {__incan_error}") },
        "invalid-utf8",
    );
    quote! {
        match ::std::str::from_utf8(#view) {
            Ok(__incan_text) => <#result_type>::Ok(__incan_text.to_string()),
            Err(__incan_error) => #failure,
        }
    }
}

/// Emit the replacing UTF-8 decode of a byte view; malformed sequences become U+FFFD, so the result is always `Ok`.
fn replace_decode(view: &TokenStream) -> TokenStream {
    let result_type = decode_result_type();
    quote! { <#result_type>::Ok(String::from_utf8_lossy(#view).into_owned()) }
}

/// Emit `data.decode(encoding="utf-8", errors="strict")` for `bytes`, `FrozenBytes`, and static byte receivers.
///
/// The receiver is read through `AsRef<[u8]>`, which every byte representation the IR carries implements, so one
/// emission covers `Vec<u8>`, `&'static [u8]`, and the frozen wrapper. The value is `Result[str, ValidationError]`
/// whatever the policy: `replace` never fails, but an unknown run-time label or policy still has to be reported.
pub fn emit_bytes_method(
    emitter: &IrEmitter,
    info: &ReceiverInfo,
    kind: &BytesMethodKind,
    args: &[IrCallArg],
) -> Result<TokenStream, EmitError> {
    let r = &info.r;
    match kind {
        BytesMethodKind::Decode => {
            let bound = TextCodecCallArgs::from_call_args(args);
            let view = quote! { <_ as AsRef<[u8]>>::as_ref(&#r) };
            let decoded = match literal_label(bound.errors).map(DecodeErrorsPolicy::from_label) {
                // The typechecker admitted the literal, so an unknown spelling cannot reach this arm.
                Some(Some(DecodeErrorsPolicy::Replace)) => replace_decode(&view),
                Some(_) => strict_decode(&view),
                None => match bound.errors {
                    None => strict_decode(&view),
                    Some(policy) => {
                        let policy_tokens = emitter.emit_expr(policy)?;
                        let strict = DecodeErrorsPolicy::Strict.as_str();
                        let replace = DecodeErrorsPolicy::Replace.as_str();
                        let strict_body = strict_decode(&view);
                        let replace_body = replace_decode(&view);
                        let unknown_policy = decode_failure(
                            quote! {
                                format!("bytes.decode() errors must be \"strict\" or \"replace\", got '{__incan_policy}'")
                            },
                            "unknown-errors-policy",
                        );
                        quote! {
                            match <_ as AsRef<str>>::as_ref(&#policy_tokens) {
                                #strict => #strict_body,
                                #replace => #replace_body,
                                __incan_policy => #unknown_policy,
                            }
                        }
                    }
                },
            };
            guard_runtime_encoding_label(emitter, "bytes.decode", bound.encoding, CodecFailure::Err, decoded)
        }
    }
}

/// Emit known string methods for string-like receivers.
pub fn emit_string_method(
    emitter: &IrEmitter,
    info: &ReceiverInfo,
    kind: &StringMethodKind,
    args: &[TypedExpr],
    call_args: &[IrCallArg],
) -> Result<TokenStream, EmitError> {
    let r_borrow = &info.r_borrow;

    match kind {
        StringMethodKind::Encode => emit_str_encode(emitter, info, call_args),
        StringMethodKind::Upper => Ok(quote! { incan_std_core::strings::str_upper(#r_borrow) }),
        StringMethodKind::Lower => Ok(quote! { incan_std_core::strings::str_lower(#r_borrow) }),
        StringMethodKind::Strip => Ok(quote! { incan_std_core::strings::str_strip(#r_borrow) }),
        StringMethodKind::Len => Ok(quote! { incan_std_core::strings::str_len(#r_borrow) }),
        StringMethodKind::Split => {
            let sep = if let Some(arg) = args.first() {
                let a = emitter.emit_expr(arg)?;
                quote! { Some(&#a) }
            } else {
                quote! { None::<&str> }
            };
            Ok(quote! { incan_std_core::strings::str_split(#r_borrow, #sep) })
        }
        StringMethodKind::Replace => {
            if args.len() >= 2 {
                let pattern = emitter.emit_expr(&args[0])?;
                let replacement = emitter.emit_expr(&args[1])?;
                Ok(quote! { incan_std_core::strings::str_replace(#r_borrow, &#pattern, &#replacement) })
            } else {
                Ok(quote! { (*#r_borrow).to_string() })
            }
        }
        StringMethodKind::Join => {
            if let Some(arg) = args.first() {
                let items = emitter.emit_expr(arg)?;
                Ok(quote! { incan_std_core::strings::str_join(#r_borrow, &#items) })
            } else {
                Ok(quote! { String::new() })
            }
        }
        StringMethodKind::StartsWith => {
            if let Some(arg) = args.first() {
                let a = emitter.emit_expr(arg)?;
                Ok(quote! { incan_std_core::strings::str_starts_with(#r_borrow, &#a) })
            } else {
                Ok(quote! { true })
            }
        }
        StringMethodKind::EndsWith => {
            if let Some(arg) = args.first() {
                let a = emitter.emit_expr(arg)?;
                Ok(quote! { incan_std_core::strings::str_ends_with(#r_borrow, &#a) })
            } else {
                Ok(quote! { true })
            }
        }
        StringMethodKind::Contains => {
            if let Some(arg) = args.first() {
                let a = emitter.emit_expr(arg)?;
                Ok(quote! { incan_std_core::strings::str_contains(#r_borrow, &#a) })
            } else {
                Ok(quote! { false })
            }
        }
    }
}

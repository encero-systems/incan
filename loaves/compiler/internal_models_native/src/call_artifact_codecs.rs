//! Grammar for plain values emitted by generated checker call-artifact projections.

/// A generated call-artifact row's grammar: one checked type, one `true` flag, one checked identity, or one resolved
/// operator call. Method rows use their own grammar, so the method codec admits no plain values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallArtifactCodec {
    SpanType,
    SpanSet,
    SpanIdentity,
    Operator,
    Method,
}

impl CallArtifactCodec {
    /// Return the stable string spelling of this codec.
    pub fn value(&self) -> String {
        match self {
            Self::SpanType => "span-type",
            Self::SpanSet => "span-set",
            Self::SpanIdentity => "span-identity",
            Self::Operator => "operator",
            Self::Method => "method",
        }
        .to_owned()
    }

    /// Classify a codec by its value-enum spelling.
    pub fn from_value(value: &str) -> Option<Self> {
        match value {
            "span-type" => Some(Self::SpanType),
            "span-set" => Some(Self::SpanSet),
            "span-identity" => Some(Self::SpanIdentity),
            "operator" => Some(Self::Operator),
            "method" => Some(Self::Method),
            _ => None,
        }
    }

    /// Type rows use the checked-type grammar; the other codecs admit these exact flat shapes.
    pub fn admits_plain_values(&self, values: &[String]) -> bool {
        match (self, values) {
            (Self::SpanSet, [flag]) => flag == "true",
            (Self::SpanIdentity, [identity]) => !identity.is_empty(),
            (Self::Operator, [kind, method]) => OperatorKind::from_value(kind).is_some() && !method.is_empty(),
            _ => false,
        }
    }
}

/// Operator spelling and enum value are specified together for both directions.
macro_rules! operator_kinds {
    ($($kind:ident = $text:literal),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum OperatorKind { $($kind),+ }

        impl OperatorKind {
            /// Recognize the exact spelling supplied by the frontend.
            pub fn from_value(value: &str) -> Option<Self> {
                match value { $($text => Some(Self::$kind),)+ _ => None }
            }

            /// Return the record spelling of this operator.
            pub fn value(&self) -> String {
                match self { $(Self::$kind => $text,)+ }.to_owned()
            }
        }
    }
}
operator_kinds! {
    Binary = "binary", Unary = "unary", Index = "index", Slice = "slice", IndexAssign = "index-assign",
    Truthiness = "truthiness", Len = "len", Contains = "contains", Call = "call",
}

/// Identify the generated projection for one field; hand-coded composite fields have no entry.
pub fn call_artifact_codec(selector: &str) -> Option<CallArtifactCodec> {
    match selector {
        "body_ir_factory_owner_types"
        | "body_ir_constructor_types"
        | "inferred_constructor_types"
        | "inferred_call_result_types"
        | "race_arm_binding_types" => Some(CallArtifactCodec::SpanType),
        "body_ir_factory_result_conflicts"
        | "mutable_receiver_method_calls"
        | "closure_holding_params"
        | "closure_returning_callables"
        | "capturing_callable_bindings" => Some(CallArtifactCodec::SpanSet),
        "body_ir_string_slice_members"
        | "body_ir_self_result_methods"
        | "body_ir_async_call_targets"
        | "body_ir_native_argument_identities"
        | "body_ir_native_codec_members"
        | "source_display_methods" => Some(CallArtifactCodec::SpanIdentity),
        "resolved_method_calls" => Some(CallArtifactCodec::Method),
        "resolved_operator_calls" => Some(CallArtifactCodec::Operator),
        _ => None,
    }
}

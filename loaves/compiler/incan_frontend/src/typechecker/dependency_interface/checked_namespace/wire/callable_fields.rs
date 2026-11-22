//! Checked callable product fields shared with the Incan graph codec at the private compiler boundary.
use super::*;
use crate::ast::Span;
use crate::library_manifest::FunctionExport;
use incan_semantics_core::stable_identity::{DeclarationSignature, StableDeclarationId, StableDeclarationLocation};
use incan_semantics_core::{CanonicalSymbolId, SymbolOrigin};

pub(crate) struct CallableInventory {
    pub(crate) namespace: Vec<(Span, String)>,
    pub(crate) environment: Vec<(Span, String)>,
    pub(crate) origin: SymbolOrigin,
    pub(crate) callables: Vec<(CallableHeader, String)>,
    pub(crate) entries: Vec<CallableOwner>,
}

model! { CallableInventory {
    namespace: 0x9359f3bb2efa,
    environment: 0xba5285161ba6,
    origin: 0x181fdd46fc4a,
    callables: 0xdc2316f8d437,
    entries: 0x87d05cd08817,
} }

pub(crate) struct CallableHeader {
    pub(crate) identity: StableDeclarationId,
    pub(crate) canonical: CanonicalSymbolId,
    pub(crate) is_public: bool,
    pub(crate) mutation: Vec<Option<bool>>,
    pub(crate) package: String,
    pub(crate) version: String,
    pub(crate) source_path: Vec<String>,
    pub(crate) replay_identity: CanonicalSymbolId,
    pub(crate) function: FunctionExport,
}

model! { CallableHeader {
    identity: 0x689f6a627384,
    canonical: 0x0deeb8fa1dbb,
    is_public: 0x2a56e0445875,
    mutation: 0xd3336a334a5e,
    package: 0xbc4a71180870,
    version: 0x5ca4f3850ccc,
    source_path: 0x857dc6ebc7d2,
    replay_identity: 0xcd2a7c4b4b13,
    function: 0x78f9ac018e55,
} }

pub(crate) struct CallableOwner {
    pub(crate) header: CallableHeader,
    pub(crate) signature: String,
    pub(crate) meaning: String,
    pub(crate) header_source: String,
    pub(crate) source: String,
    pub(crate) reads: Vec<CallableRead>,
    pub(crate) declaration: Vec<u8>,
    pub(crate) local: Vec<u8>,
    pub(crate) final_value: Vec<u8>,
    pub(crate) contexts: Vec<((CanonicalSymbolId, CanonicalSymbolId), u32)>,
    pub(crate) body: Vec<u8>,
}

model! { CallableOwner {
    header: 0x1e0584a25d9f,
    signature: 0x1a2fc26dc7ea,
    meaning: 0xe409bfc6fe79,
    header_source: 0x85b35eaee32a,
    source: 0x41cf6794ba42,
    reads: 0x3566cf23ecb6,
    declaration: 0xabcd0ee1fc2c,
    local: 0x25bf8e1a2393,
    final_value: 0x75203e01aad7,
    contexts: 0x67183e4ab671,
    body: 0x230d8358dc8e,
} }

pub(crate) struct CallableRead {
    pub(crate) name: String,
    pub(crate) origin: SymbolOrigin,
    pub(crate) candidates: Vec<(StableDeclarationId, String)>,
    pub(crate) signatures: Vec<(CanonicalSymbolId, DeclarationSignature)>,
    pub(crate) inputs: Vec<(Span, String)>,
}

model! { CallableRead {
    name: 0x82a3537ff0db,
    origin: 0x181fdd46fc4a,
    candidates: 0xba159dbf9d2f,
    signatures: 0x0316e1809318,
    inputs: 0x32381523e631,
} }

model! { FunctionExport {
    ownership: 0x01b8016fdce4,
    name: 0x82a3537ff0db,
    emitted_name: 0xd41a29ea2171,
    type_params: 0x75e7e9dead7e,
    params: 0xa20b52fae57c,
    return_type: 0x407ddf95ec74,
    is_async: 0x4e13cb86ec33,
} }

model! { StableDeclarationId {
    namespace: 0x9359f3bb2efa,
    origin: 0x181fdd46fc4a,
    declaration_name: 0xd63bdabe07a6,
    kind: 0x0b27c158feb3,
    location: 0xe6eaea18e885,
    signature: 0x1a2fc26dc7ea,
} }

sum! { StableDeclarationLocation {
    ModuleLevel = 0x609502212eb7,
    Nested = 0xa5e4744f2cd8 { owner: 0x4c1029697ee3, binding_ordinal: 0x5025259b76b5 },
} }

impl Project for DeclarationSignature {
    fn project(&self) -> Cell {
        Cell::Text(self.as_str().to_owned())
    }
    fn restore(value: Cell) -> Result<Self, String> {
        use incan_semantics_core::stable_identity::source_signature::SOURCE_SIGNATURE_PROFILE;
        use serde::Deserialize;
        use serde::de::value::{Error, SeqDeserializer};

        let spelling = String::restore(value)?;
        let digest = spelling
            .strip_prefix(SOURCE_SIGNATURE_PROFILE)
            .and_then(|rest| rest.strip_prefix(":sha256:"))
            .ok_or_else(|| "unknown callable declaration signature profile".to_owned())?;
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("invalid callable declaration signature digest".into());
        }
        // The kernel exposes its scalar as a one-field tuple struct through its existing typed Serde contract.
        Self::deserialize(SeqDeserializer::<_, Error>::new(std::iter::once(spelling)))
            .map_err(|error| error.to_string())
    }
}

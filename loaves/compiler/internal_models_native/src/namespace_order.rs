//! Namespace ranks, checking bases and import closures for ranked checking.

pub const CHECKING_BASIS: &str = "incan.compiler.checking-basis/2";
pub const IMPORT_ORDER_FILE: &str = "import-closures.crumb";

/// Strict predecessor count and rank evidence for one module or one mutually importing group.
///
/// Defaults: `rank` zero, `evidence` `"compiled-check"` (older and partial publications have no certificate for
/// independent scheduling), `members` and `certificate` empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamespaceOrder {
    pub rank: i64,
    pub evidence: String,
    pub members: Vec<String>,
    pub certificate: Vec<u8>,
}

impl Default for NamespaceOrder {
    /// The reference model's field defaults.
    fn default() -> Self {
        Self {
            rank: 0,
            evidence: "compiled-check".to_owned(),
            members: Vec::new(),
            certificate: Vec::new(),
        }
    }
}

//! Payload admissions that transfer across a rename while the filesystem observation still matches.

/// Proof transferable across a rename while the filesystem observation still matches. `facts` and `refusal` default
/// to empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PayloadAdmission {
    pub identity: String,
    pub payload: String,
    pub observation: String,
    pub provenance: Vec<String>,
    pub facts: String,
    pub refusal: String,
}

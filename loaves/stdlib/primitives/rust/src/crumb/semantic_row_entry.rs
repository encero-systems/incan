//! The native entry of the public Crumb row API: one envelope admitted from the API's own inputs.
//!
//! `row_reader.read_rows` and `read_prefix` take a view, supplied descriptions, a kind registry, whether a seal is
//! required, and the caller's `Budget`. A `RowRequest` takes the same facts in host form: the view's bytes, each
//! supplied description D-encoded, each registered profile kind as a name and a record or enum shape, and the
//! allowance left in the caller's budget. Its admission reports a positioned refusal or what it admitted, and always
//! the allowance it used, so the caller charges its own budget exactly that much.

use super::columns::Budget;
use super::row_cursor::{Refusal, at};
use super::rows::{self, Meta, ReadInputs, Rows};

/// One native row-envelope admission: a positioned refusal or the admitted envelope's facts, and the allowance used.
#[derive(Clone)]
pub struct RowAdmission {
    /// Empty when the envelope is admitted; otherwise the refusal category as `BinaryFailure` spells it.
    pub category: String,
    /// The refusal's envelope offset.
    pub offset: i64,
    /// The stored field identity a refusal is in, when known.
    pub field: Option<Vec<u8>>,
    /// The enum variant identity a refusal is in, when known.
    pub variant: Option<Vec<u8>>,
    /// The exhausted caller-owned resource when a budget charge caused the refusal.
    pub cause: String,
    /// The input bytes the admitted envelope occupies.
    pub consumed: i64,
    /// True only when the complete logical seal was verified.
    pub sealed: bool,
    /// The admitted record count.
    pub records: i64,
    /// Bytes of the allowance this admission used, admitted or refused.
    pub bytes_charged: i64,
    /// Elements of the allowance this admission used, admitted or refused.
    pub elements_charged: i64,
    /// The admitted envelope's bytes and decoded rows, which `semantic_row_set.rs` reads on request.
    pub(super) data: Vec<u8>,
    pub(super) rows: Option<Rows>,
}

impl std::fmt::Debug for RowAdmission {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RowAdmission")
            .field("category", &self.category)
            .field("offset", &self.offset)
            .field("consumed", &self.consumed)
            .field("records", &self.records)
            .finish_non_exhaustive()
    }
}

impl RowAdmission {
    /// Whether the envelope was admitted.
    pub fn admitted(&self) -> bool {
        self.category.is_empty()
    }
}

/// The inputs of one public row read, gathered by the Incan caller before it asks for the admission.
#[derive(Clone, Debug)]
pub struct RowRequest {
    data: Vec<u8>,
    supplied: Vec<Vec<u8>>,
    kinds: Vec<(String, bool)>,
    require_sealed: bool,
    exact: bool,
    allowance: (u64, u64, u64),
}

impl RowRequest {
    /// A request to admit one envelope from `data`. An exact read refuses any bytes after it, as `read_rows` does; a
    /// prefix read, as `read_prefix` does, reports how many bytes it occupies. The allowance starts empty.
    pub fn new(data: Vec<u8>, require_sealed: bool, exact: bool) -> Self {
        Self {
            data,
            supplied: Vec::new(),
            kinds: Vec::new(),
            require_sealed,
            exact,
            allowance: (0, 0, 0),
        }
    }

    /// Supply one complete reference description, D-encoded.
    pub fn supply(&mut self, description: Vec<u8>) {
        self.supplied.push(description);
    }

    /// Register one profile kind with its record (true) or enum (false) shape.
    pub fn register(&mut self, kind: String, record: bool) {
        self.kinds.push((kind, record));
    }

    /// Set the allowance left in the caller's budget: bytes, elements and nesting depth. A negative amount is none.
    pub fn allow(&mut self, bytes: i64, elements: i64, depth: i64) {
        let amount = |value: i64| u64::try_from(value).unwrap_or(0);
        self.allowance = (amount(bytes), amount(elements), amount(depth));
    }

    /// Admit the envelope, charging only this request's allowance.
    pub fn admit(&self) -> RowAdmission {
        self.admit_with_descriptions(Some(&self.supplied))
    }

    fn admit_with_descriptions(&self, encoded_descriptions: Option<&[Vec<u8>]>) -> RowAdmission {
        let (bytes, elements, depth) = self.allowance;
        let mut budget = Budget::new(bytes, elements, depth);
        let result = match self.supplied_descriptions() {
            Some(supplied) => {
                let inputs = ReadInputs {
                    supplied: &supplied,
                    encoded_descriptions,
                    kinds: &self.kinds,
                    require_sealed: self.require_sealed,
                    exact: self.exact,
                };
                rows::read_rows(&self.data, &inputs, &mut budget)
            }
            None => Err(at("invalid_description", 0)),
        };
        let (bytes_left, elements_left) = budget.left();
        admission(
            result,
            &self.data,
            bytes - bytes_left,
            elements - elements_left,
            budget.cause(),
        )
    }

    /// Decode each supplied description outside the caller's allowance; the caller already holds them as values.
    fn supplied_descriptions(&self) -> Option<Vec<Meta>> {
        self.supplied
            .iter()
            .map(|encoded| rows::read_description(encoded))
            .collect()
    }
}

/// The admission's facts from a read result and the allowance it used.
fn admission(
    result: Result<Rows, Refusal>,
    data: &[u8],
    bytes: u64,
    elements: u64,
    cause: Option<&str>,
) -> RowAdmission {
    let count = |value: usize| i64::try_from(value).unwrap_or(i64::MAX);
    let charged = |value: u64| i64::try_from(value).unwrap_or(i64::MAX);
    let mut admission = RowAdmission {
        category: String::new(),
        offset: 0,
        field: None,
        variant: None,
        cause: String::new(),
        consumed: 0,
        sealed: false,
        records: 0,
        bytes_charged: charged(bytes),
        elements_charged: charged(elements),
        data: Vec::new(),
        rows: None,
    };
    match result {
        Ok(rows) => {
            admission.consumed = count(rows.consumed);
            admission.sealed = rows.sealed;
            admission.records = count(rows.records.len());
            admission.data = data[..rows.consumed].to_vec();
            admission.rows = Some(rows);
        }
        Err(refusal) => {
            admission.category = refusal.category.to_string();
            admission.offset = count(refusal.offset);
            admission.field = refusal.field;
            admission.variant = refusal.variant;
            if refusal.category == "limit_exceeded" {
                admission.cause = cause.unwrap_or_default().to_string();
            }
        }
    }
    admission
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The corpus's `bytes-enum-inline` envelope: one inline enum description and one row of its only variant.
    const ENUM_INLINE: &str = "43524d420101007401006b3f0604046b696e640304456e756d076e6f6d696e616c04071ddb0b3ce6d3000470617468030d6578616d706c652e53746174650a70726f7065727469657306000100282606030269640406013f54400c82046e616d65030552656164790a70726f70657274696573060000010003000100";

    /// The corpus's `bytes-empty-reference` envelope and its one supplied description, in corpus notation.
    const EMPTY_REFERENCE: &str = "43524d420101002f0101071ddb0b3ce6d3009e98c15038948f7d341f1889af1dbdfa8509d3e6f8f6bb8ec783cce0a6d723770100020100";
    const EMPTY_DESCRIPTION: &str = r#"{"kind":"map","entries":[[{"kind":"string","value":"nominal"},{"kind":"binary","hex":"1ddb0b3ce6d300"}],[{"kind":"string","value":"kind"},{"kind":"string","value":"Record"}],[{"kind":"string","value":"path"},{"kind":"string","value":"example.Value"}],[{"kind":"string","value":"fields"},{"kind":"list","items":[]}],[{"kind":"string","value":"properties"},{"kind":"map","entries":[]}]]}"#;

    fn bytes(hex: &str) -> Vec<u8> {
        super::super::columns::hex(hex).expect("test hex")
    }

    fn request(data: Vec<u8>, exact: bool) -> RowRequest {
        let mut request = RowRequest::new(data, false, exact);
        request.allow(16_777_216, 1_048_576, 128);
        request
    }

    #[test]
    fn an_exact_read_refuses_trailing_bytes_and_a_prefix_read_reports_its_length() {
        let mut data = bytes(ENUM_INLINE);
        let length = data.len();
        data.push(0);
        let exact = request(data.clone(), true).admit();
        assert_eq!(
            (exact.category.as_str(), exact.offset),
            ("trailing_bytes", length as i64)
        );
        let prefix = request(data, false).admit();
        assert!(prefix.admitted(), "prefix read refused: {}", prefix.category);
        assert_eq!(
            (prefix.consumed, prefix.records, prefix.sealed),
            (length as i64, 1, false)
        );
    }

    #[test]
    fn supplied_descriptions_arrive_d_encoded() {
        let missing = request(bytes(EMPTY_REFERENCE), true).admit();
        assert_eq!((missing.category.as_str(), missing.offset), ("missing_description", 9));
        let notation = serde_json::from_str(EMPTY_DESCRIPTION).expect("description notation");
        let description = rows::notation(&notation).expect("description");
        let mut encoded = Vec::new();
        let mut unlimited = Budget::new(u64::MAX, u64::MAX, u64::MAX);
        rows::encode_metadata(&description, &mut encoded, &mut unlimited).expect("D encoding");
        let mut supplied = request(bytes(EMPTY_REFERENCE), true);
        supplied.supply(encoded);
        let admitted = supplied.admit();
        assert!(admitted.admitted(), "reference refused: {}", admitted.category);
        assert_eq!(admitted.records, 1);
        let mut malformed = request(bytes(EMPTY_REFERENCE), true);
        malformed.supply(vec![9]);
        assert_eq!(malformed.admit().category, "invalid_description");
    }

    #[test]
    fn referenced_dictionaries_preserve_budgets_refusals_and_row_validation() -> Result<(), String> {
        let notation = serde_json::from_str(EMPTY_DESCRIPTION).map_err(|error| error.to_string())?;
        let description = rows::notation(&notation).ok_or("description")?;
        let mut encoded = Vec::new();
        let mut budget = Budget::new(u64::MAX, u64::MAX, u64::MAX);
        rows::encode_metadata(&description, &mut encoded, &mut budget)?;
        let mut reference = request(bytes(EMPTY_REFERENCE), true);
        reference.supply(encoded.clone());
        reference.supply(encoded);
        let first = reference.admit();
        let second = reference.admit();
        let dictionary = |a: RowAdmission| a.rows.map(|rows| rows.dictionary).ok_or("rows missing");
        let byte_charge = first.bytes_charged;
        let element_charge = first.elements_charged;
        assert!(std::sync::Arc::ptr_eq(&dictionary(first)?, &dictionary(second)?));

        let compare = |request: &RowRequest| {
            let current = request.admit();
            let independent = request.admit_with_descriptions(None);
            assert_eq!(current.category, independent.category);
            assert_eq!(current.offset, independent.offset);
            assert_eq!(current.cause, independent.cause);
            assert_eq!(current.field, independent.field);
            assert_eq!(current.variant, independent.variant);
            assert_eq!(current.bytes_charged, independent.bytes_charged);
            assert_eq!(current.elements_charged, independent.elements_charged);
            assert_eq!(current.records, independent.records);
            assert_eq!(current.key(0), independent.key(0));
            current
        };
        for (bytes, elements, depth) in [
            (byte_charge, element_charge, 128),
            (byte_charge - 1, element_charge, 128),
            (byte_charge, element_charge - 1, 128),
            (byte_charge / 2, element_charge / 2, 128),
            (byte_charge, element_charge, 1),
        ] {
            let mut limited = reference.clone();
            limited.allow(bytes, elements, depth);
            compare(&limited);
        }
        let mut require_seal = reference.clone();
        require_seal.require_sealed = true;
        assert_eq!(compare(&require_seal).category, "seal_required");
        let mut damaged_row = reference.clone();
        if let Some(last) = damaged_row.data.last_mut() {
            *last = 255;
        }
        assert!(!compare(&damaged_row).admitted());
        let mut malformed_supply = reference.clone();
        malformed_supply.supplied[1] = vec![9];
        assert_eq!(compare(&malformed_supply).category, "invalid_description");
        let mut changed_dictionary = reference.clone();
        changed_dictionary.data[20] ^= 1;
        assert_eq!(compare(&changed_dictionary).category, "description_mismatch");
        let mut registered = reference.clone();
        registered.register("Another".to_string(), false);
        assert!(!std::sync::Arc::ptr_eq(
            &dictionary(reference.admit())?,
            &dictionary(compare(&registered))?
        ));
        Ok(())
    }

    #[test]
    fn a_registered_profile_kind_takes_its_registered_shape() {
        // The corpus's `bytes-empty-inline` envelope with its kind `Record` respelled `Sample`.
        let profile = "43524d420101004b010043410604046b696e64030653616d706c65076e6f6d696e616c04071ddb0b3ce6d3000470617468030d6578616d706c652e56616c75650a70726f706572746965730600000100020100";
        let unknown = request(bytes(profile), true).admit();
        assert_eq!((unknown.category.as_str(), unknown.offset), ("invalid_description", 11));
        let mut registered = request(bytes(profile), true);
        registered.register("Sample".to_string(), true);
        let admitted = registered.admit();
        assert!(admitted.admitted(), "registered kind refused: {}", admitted.category);
        assert_eq!(admitted.records, 1);
    }

    #[test]
    fn the_admission_reports_exactly_the_allowance_it_used() {
        let first = request(bytes(ENUM_INLINE), true).admit();
        assert!(first.admitted());
        assert!(first.bytes_charged > 0 && first.elements_charged > 0);
        let mut exact = RowRequest::new(bytes(ENUM_INLINE), false, true);
        exact.allow(first.bytes_charged, first.elements_charged, 128);
        let again = exact.admit();
        assert!(
            again.admitted(),
            "the reported allowance did not suffice: {}",
            again.category
        );
        assert_eq!(
            (again.bytes_charged, again.elements_charged),
            (first.bytes_charged, first.elements_charged)
        );
        for (bytes_left, elements_left, cause) in [
            (first.bytes_charged - 1, first.elements_charged, "bytes"),
            (first.bytes_charged, first.elements_charged - 1, "elements"),
        ] {
            let mut short = RowRequest::new(bytes(ENUM_INLINE), false, true);
            short.allow(bytes_left, elements_left, 128);
            let refused = short.admit();
            assert_eq!(refused.category, "limit_exceeded");
            assert_eq!(refused.cause, cause);
            assert!(refused.bytes_charged <= bytes_left && refused.elements_charged <= elements_left);
        }
    }
}

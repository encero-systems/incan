//! Compiler row transport through the standard library's native Crumb request interface.

pub use crate::general_rows::{
    ModelField, decode_rows, encode_rows, model_description, operation_budget, required_value,
};

use crate::{CodecFault, row_shapes::Shape};
use CodecFault::{Limit, Malformed};
use internal_models_bindings::{RowAdmission, RowRead, RowRequest, RowWriteRequest};

const ELEMENTS: i64 = 4_194_304;
const DEPTH: i64 = 128;

pub(crate) struct Writer {
    request: RowWriteRequest,
    shape: &'static Shape,
    maximum: i64,
}

impl Writer {
    /// Start an unsealed writer for one referenced compiler schema.
    pub fn new(shape: &'static Shape, maximum: i64) -> Result<Self, CodecFault> {
        let mut request = RowWriteRequest::new(false);
        request.description(shape.metadata().ok_or(Limit)?.to_vec(), false);
        request.allow(maximum * 4, ELEMENTS, DEPTH);
        Ok(Self {
            request,
            shape,
            maximum,
        })
    }
    /// Start the next record of the selected model.
    pub fn record(&mut self) {
        self.request.record(self.shape.selector.to_vec());
        self.node(14, self.shape.fields.len() as i64);
    }
    /// Append a collection or option tag and its node count.
    fn node(&mut self, code: i64, count: i64) {
        self.request
            .node(code, count, false, 0, 0, 0, 0, Vec::new(), Vec::new(), String::new());
    }
    /// Introduce a present canonical field by its required six-byte identity.
    fn field(&mut self, id: &[u8]) {
        self.request
            .node(18, 0, true, 0, 0, 0, 0, id.to_vec(), Vec::new(), "required".into());
    }
    /// Move an owned string into the native row request.
    pub fn text_node(&mut self, value: String) {
        self.request
            .node(7, 0, false, 0, 0, 0, 0, Vec::new(), Vec::new(), value);
    }
    /// Write a present string field.
    pub fn text(&mut self, id: &[u8], value: &str) {
        self.field(id);
        self.text_node(value.to_owned());
    }
    /// Write the presence marker and optional string payload.
    pub fn optional_text(&mut self, id: &[u8], value: Option<&str>) {
        self.field(id);
        if let Some(s) = value {
            self.node(1, 1);
            self.text_node(s.to_owned());
        } else {
            self.node(0, 0);
        }
    }
    /// Write a byte field owned by the encoding request.
    pub fn binary(&mut self, id: &[u8], value: &[u8]) {
        self.field(id);
        self.request
            .node(8, 0, false, 0, 0, 0, 0, value.to_vec(), Vec::new(), String::new());
    }
    /// Start a text list of the declared length.
    pub fn list(&mut self, id: &[u8], len: usize) -> Result<(), CodecFault> {
        self.field(id);
        self.node(10, i64::try_from(len).map_err(|_| Limit)?);
        Ok(())
    }
    /// Return complete encoded rows within the caller’s byte boundary.
    pub fn finish(self) -> Result<Vec<u8>, CodecFault> {
        let result = self.request.write();
        if !result.category.is_empty() || result.data.len() as u64 > self.maximum as u64 {
            return Err(Limit);
        }
        Ok(result.data)
    }
}

pub(crate) struct Rows {
    admitted: RowAdmission,
    bytes_left: i64,
    elements_left: i64,
}

impl Rows {
    /// Admit an exact envelope with the permitted schemas and retain the remaining read allowance.
    pub fn admit(data: &[u8], shapes: &[&Shape], maximum: i64) -> Result<Self, CodecFault> {
        if data.len() as u64 > maximum as u64 {
            return Err(Malformed);
        }
        let mut request = RowRequest::new(data.to_vec(), false, true);
        for shape in shapes {
            request.supply(shape.metadata().ok_or(Malformed)?.to_vec())
        }
        request.allow(maximum * 4, ELEMENTS, DEPTH);
        let admitted = request.admit();
        if !admitted.admitted() {
            return Err(Malformed);
        }
        Ok(Self {
            bytes_left: maximum * 4 - admitted.bytes_charged,
            elements_left: ELEMENTS - admitted.elements_charged,
            admitted,
        })
    }
    /// Return the number of rows authenticated by admission.
    pub fn len(&self) -> i64 {
        self.admitted.records
    }
    /// Read and charge one logical row before interpreting it as a compiler model.
    fn read_native(&mut self, index: i64) -> Result<RowRead, CodecFault> {
        let read = self
            .admitted
            .read_record(index, self.bytes_left, self.elements_left, DEPTH);
        if !read.succeeded() {
            return Err(Malformed);
        }
        self.bytes_left -= read.bytes_charged;
        self.elements_left -= read.elements_charged;
        Ok(read)
    }

    /// The original admits every row before authenticating owner data. A later native read refusal takes precedence.
    pub fn remaining_reads(&mut self, start: i64) -> Result<(), CodecFault> {
        for index in start..self.len() {
            self.read_native(index)?;
        }
        Ok(())
    }

    /// Materialize one native row under the remaining shared budget.
    pub fn row(&mut self, index: i64) -> Result<(Vec<u8>, Fields), CodecFault> {
        let mut read = self.read_native(index)?;
        if read.next() != 14 {
            return Err(Malformed);
        }
        Ok((self.admitted.key(index), Fields(read)))
    }
}

pub(crate) struct Fields(RowRead);
impl Fields {
    /// Check the exact declared field count before consuming the record.
    pub fn count(&self, expected: i64) -> Result<(), CodecFault> {
        if self.0.count() == expected {
            Ok(())
        } else {
            Err(Malformed)
        }
    }
    /// Advance one node and require its expected semantic tag.
    fn tagged(&mut self, tag: i64) -> Result<(), CodecFault> {
        if self.0.next() == tag { Ok(()) } else { Err(Malformed) }
    }
    /// Read a single string field from the current row.
    pub fn text(&mut self) -> Result<String, CodecFault> {
        self.tagged(18)?;
        self.tagged(7)?;
        Ok(self.0.text())
    }
    /// Read a single byte-string field from the current row.
    pub fn binary(&mut self) -> Result<Vec<u8>, CodecFault> {
        self.tagged(18)?;
        self.tagged(8)?;
        Ok(self.0.bytes())
    }
    /// Read an absent or nonempty identity from its option field.
    pub fn optional_text(&mut self) -> Result<Option<String>, CodecFault> {
        self.tagged(18)?;
        match self.0.next() {
            0 => Ok(None),
            1 => {
                self.tagged(7)?;
                let s = self.0.text();
                if s.is_empty() { Err(Malformed) } else { Ok(Some(s)) }
            }
            _ => Err(Malformed),
        }
    }
    /// Read every string from a declared list in wire order.
    pub fn texts(&mut self) -> Result<Vec<String>, CodecFault> {
        self.tagged(18)?;
        self.tagged(10)?;
        (0..self.0.count())
            .map(|_| {
                self.tagged(7)?;
                Ok(self.0.text())
            })
            .collect()
    }
}

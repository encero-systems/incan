//! Borrowed token cursor for the versioned input ownership grammar.

use crate::CodecFault::{self, Malformed};
use crate::checker_field_grammar::{CanonicalField, at, number, parameters_end, type_end};
use std::cmp::Ordering;

type Checked<T> = Result<T, CodecFault>;

struct Reader<'a> {
    tokens: &'a [String],
    pos: usize,
    strict: bool,
    max_node: usize,
}
impl<'a> Reader<'a> {
    /// Read one borrowed token from the input.
    fn take(&mut self) -> Checked<&'a str> {
        let value = at(self.tokens, self.pos)?;
        self.pos += 1;
        Ok(value)
    }
    /// Validate an enumerated primitive without allocating a copy.
    fn tag(&mut self, choices: &[&str]) -> Checked<&'a str> {
        let token = self.take()?;
        if choices.contains(&token) {
            Ok(token)
        } else {
            Err(Malformed)
        }
    }
    /// Host-independent decimal primitive.
    fn number(&mut self) -> Checked<usize> {
        number(self.take()?)
    }
    /// A collection count cannot exceed the remaining number of tokens.
    fn count(&mut self) -> Checked<usize> {
        let count = self.number()?;
        if count <= self.tokens.len() - self.pos {
            Ok(count)
        } else {
            Err(Malformed)
        }
    }
    /// Handles are opaque and nonempty.
    fn handle(&mut self) -> Checked<()> {
        if self.take()?.is_empty() {
            Err(Malformed)
        } else {
            Ok(())
        }
    }
    /// Consume the presence marker which precedes an optional grammar production.
    fn present(&mut self) -> Checked<bool> {
        Ok(self.tag(&["none", "some"])? == "some")
    }
    /// Consume one exact boolean token.
    fn flag(&mut self) -> Checked<()> {
        self.tag(&["true", "false"])?;
        Ok(())
    }
    /// Consume a loan or successor index under the selected graph policy.
    fn index(&mut self, limit: usize) -> Checked<()> {
        let index = self.number()?;
        if self.strict && index >= limit {
            Err(Malformed)
        } else {
            Ok(())
        }
    }
    /// An absent loan has no index.
    fn optional_index(&mut self, limit: usize) -> Checked<()> {
        if self.present()? {
            self.index(limit)?;
        }
        Ok(())
    }
    /// Node bounds become available later. Only the largest referenced index is needed to validate all of them.
    fn node(&mut self) -> Checked<()> {
        let index = self.number()?;
        self.max_node = self.max_node.max(index);
        Ok(())
    }
    /// Read one checked type starting at the current token.
    fn value_type(&mut self) -> Checked<()> {
        self.pos = type_end(self.tokens, self.pos, 0)?;
        Ok(())
    }
    /// Read an ownership-annotated storage root.
    fn root(&mut self) -> Checked<()> {
        let kind = self.tag(&["binding", "temporary"])?;
        self.handle()?;
        if kind == "temporary" {
            self.handle()?;
            self.handle()?;
        }
        Ok(())
    }
    /// Consume all projections of a storage place.
    fn place(&mut self) -> Checked<()> {
        self.root()?;
        for _ in 0..self.count()? {
            self.tag(&["field", "element"])?;
            self.handle()?;
        }
        Ok(())
    }
    /// Parse each map key once and compare the framed spelling using slices of the original tokens.
    fn map<K, V>(&mut self, key: K, mut value: V) -> Checked<()>
    where
        K: Fn(&mut Self) -> Checked<()>,
        V: FnMut(&mut Self) -> Checked<()>,
    {
        let mut previous: &[String] = &[];
        for _ in 0..self.count()? {
            let start = self.pos;
            key(self)?;
            let tokens = &self.tokens[start..self.pos];
            if framed_order(previous, tokens) != Ordering::Less {
                return Err(Malformed);
            }
            previous = tokens;
            value(self)?;
        }
        Ok(())
    }
    /// Retain the node bounds of the operand interval, including an optional source access.
    fn evaluation(&mut self) -> Checked<()> {
        self.node()?;
        self.node()?;
        if self.present()? {
            self.node()?;
        }
        Ok(())
    }
    /// Decode per-argument fields and the dispatch and completion dependencies of this call.
    fn invocation(&mut self, loans: usize, receivers: bool, results: bool, observers: bool) -> Checked<()> {
        self.handle()?;
        if results && self.present()? {
            self.value_type()?;
        }
        self.optional_index(loans)?;
        self.flag()?;
        if self.present()? {
            self.place()?;
        }
        if receivers {
            for _ in 0..self.count()? {
                self.index(loans)?;
            }
        }
        for _ in 0..self.count()? {
            self.handle()?;
            self.number()?;
            self.number()?;
            self.tag(&["inferred", "inferred_shared", "shared", "exclusive", "owned"])?;
            self.optional_index(loans)?;
            if self.present()? {
                self.place()?;
            }
            if observers {
                self.flag()?;
            }
            self.evaluation()?;
        }
        if self.present()? {
            self.evaluation()?;
        }
        self.node()?;
        for _ in 0..self.count()? {
            self.node()?;
        }
        Ok(())
    }
    /// Read the complete ownership graph, preserving version-specific validation.
    fn graph(&mut self, strict: bool, receivers: bool, results: bool, observers: bool) -> Checked<()> {
        self.strict = strict;
        self.handle()?;
        self.node()?;
        self.map(Self::root, |r| {
            r.flag()?;
            r.flag()?;
            r.flag()
        })?;
        let loans = self.count()?;
        for _ in 0..loans {
            self.place()?;
            self.tag(&["shared", "exclusive"])?;
            self.flag()?;
            self.optional_index(loans)?;
            self.handle()?;
        }
        self.map(Self::handle, |r| r.index(loans))?;
        self.map(Self::handle, |r| {
            for _ in 0..r.count()? {
                r.index(loans)?;
            }
            Ok(())
        })?;
        for _ in 0..self.count()? {
            self.handle()?;
        }
        self.map(Self::handle, |r| {
            r.handle()?;
            let source = r.present()?;
            if source {
                r.place()?;
            }
            let access = r.present()?;
            if access {
                r.node()?;
            }
            if r.strict && source != access {
                Err(Malformed)
            } else {
                Ok(())
            }
        })?;
        self.map(Self::handle, |r| {
            r.node()?;
            for _ in 0..r.count()? {
                r.handle()?;
                r.place()?;
                r.index(loans)?;
                r.node()?;
            }
            Ok(())
        })?;
        self.map(Self::handle, |r| r.invocation(loans, receivers, results, observers))?;
        self.map(Self::handle, |r| {
            r.handle()?;
            for _ in 0..r.count()? {
                r.handle()?;
            }
            r.optional_index(loans)?;
            for _ in 0..r.count()? {
                r.node()?;
            }
            Ok(())
        })?;
        let nodes = self.count()?;
        if strict && self.max_node >= nodes {
            return Err(Malformed);
        }
        for _ in 0..nodes {
            match self.tag(&["nothing", "borrow", "access"])? {
                "borrow" => self.index(loans)?,
                "access" => {
                    self.place()?;
                    self.tag(&["read", "write", "move", "replace"])?;
                    self.optional_index(loans)?;
                }
                _ => {}
            }
            for _ in 0..self.count()? {
                self.index(nodes)?;
            }
            self.handle()?;
        }
        Ok(())
    }
}

/// Compare the exact sequence of decimal UTF-8 lengths, colons and token bytes without building joined keys.
fn framed_order(left: &[String], right: &[String]) -> Ordering {
    for (a, b) in left.iter().zip(right) {
        let order = if a.len() == b.len() {
            a.cmp(b)
        } else {
            let (mut left_buffer, mut right_buffer) = ([0; 21], [0; 21]);
            length_prefix(a.len(), &mut left_buffer).cmp(length_prefix(b.len(), &mut right_buffer))
        };
        if order != Ordering::Equal {
            return order;
        }
    }
    left.len().cmp(&right.len())
}

/// Return the decimal byte length and trailing colon. A 64-bit length occupies at most twenty digits.
fn length_prefix(mut n: usize, buffer: &mut [u8; 21]) -> &[u8] {
    buffer[20] = b':';
    let mut start = 20;
    loop {
        start -= 1;
        buffer[start] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            return &buffer[start..];
        }
    }
}

/// Validate one input-copy, input-call, input-local or input-flow record and return the first unconsumed token.
pub(crate) fn input_field_end(field: &CanonicalField) -> Checked<usize> {
    if field.node.is_empty() {
        return Err(Malformed);
    }
    let mut r = Reader {
        tokens: &field.values,
        pos: 0,
        strict: true,
        max_node: 0,
    };
    let version = r.tag(&["1", "2", "5"])?;
    let family = (field.family.as_str(), field.selector.as_str());
    let input_flow = family == ("input-flow", "graph");
    let input_copy = family == ("input-copy", "types");
    if (input_copy && version != "2") || (input_flow && version != "5") {
        return Err(Malformed);
    }
    if !input_copy && !input_flow && version != "1" {
        return Err(Malformed);
    }
    match family {
        ("input-copy", "types") => {
            r.handle()?;
            r.map(Reader::value_type, Reader::flag)?;
        }
        ("input-call", "contract") => {
            r.tag(&["explicit", "ordinary", "indirect"])?;
            if r.present()? {
                r.handle()?;
            }
            r.handle()?;
            r.tag(&["none", "shared", "exclusive"])?;
            r.pos = parameters_end(r.tokens, r.pos, 0)?;
        }
        ("input-local", "mode") => {
            r.handle()?;
            r.handle()?;
            r.tag(&["Borrowed", "BorrowedMut", "Owned", "Reference"])?;
            r.flag()?;
            r.handle()?;
            r.value_type()?;
        }
        ("input-flow", "graph") => match r.tag(&["checked", "invalid", "unavailable"])? {
            "unavailable" => {
                r.handle()?;
                r.handle()?;
                r.handle()?;
            }
            state => {
                r.graph(state == "checked", true, true, true)?;
                if state == "invalid" {
                    r.handle()?;
                }
            }
        },
        _ => return Err(Malformed),
    }
    Ok(r.pos)
}

/// Read the selected method, its trait dispatch and all implementation-bound associated types.
pub(crate) fn method_call_end(values: &[String]) -> Checked<usize> {
    let mut r = Reader {
        tokens: values,
        pos: 0,
        strict: true,
        max_node: 0,
    };
    r.handle()?;
    r.tag(&["trait"])?;
    r.handle()?;
    if r.tag(&["some", "none"])? == "some" {
        for _ in 0..r.count()? {
            r.handle()?;
        }
    }
    for _ in 0..r.count()? {
        r.value_type()?;
    }
    for _ in 0..r.count()? {
        r.handle()?;
        for _ in 0..r.count()? {
            r.handle()?;
            for _ in 0..r.count()? {
                r.value_type()?;
            }
            for _ in 0..r.count()? {
                r.handle()?;
                r.value_type()?;
            }
            r.tag(&["standard", "rust-capability", "source-callable"])?;
        }
    }
    r.flag()?;
    Ok(r.pos)
}

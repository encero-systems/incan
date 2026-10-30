//! Complete per-body cold comparisons with the original shared-output checker.

use super::TypeChecker;
use crate::ast::Span;

impl TypeChecker {
    /// Capture all local fact families and ordered pending decisions after a body. These verification snapshots never
    /// become producer inputs; only unordered Debug containers are sorted and every ordered element is retained.
    pub(in crate::typechecker) fn observe_body_local(&mut self, span: Span) {
        let value = format!(
            "{:?}",
            (
                &self.type_info.expressions,
                &self.type_info.references,
                &self.type_info.calls,
                &self.type_info.protocols,
                &self.type_info.rust,
                &self.type_info.traits,
                &self.type_info.c_abi,
                (
                    &self.type_info.body_ir_iterator_sum_items,
                    &self.type_info.body_ir_newtypes,
                ),
                &self.mut_params,
                &self.pending_uncopyable_dict_lookups,
                &self.errors,
                &self.warnings,
            )
        );
        self.body_local_observations.push((span, canonical_debug(&value)));
    }
}

/// Sort unordered Debug containers while preserving variants, sequences, scalars and quoted bytes. This exact test
/// comparator is never used for production identity, storage or semantic projection.
fn canonical_debug(value: &str) -> String {
    debug_container(&mut value.chars().peekable(), None, false)
}

/// Consume one delimited Debug container, sorting maps, sets and named fields while retaining every payload value.
fn debug_container(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, close: Option<char>, sort: bool) -> String {
    let mut parts = Vec::new();
    let mut part = String::new();
    while let Some(character) = chars.next() {
        match character {
            '"' | '\'' => {
                part.push(character);
                while let Some(quoted) = chars.next() {
                    part.push(quoted);
                    if quoted == '\\' {
                        if let Some(escaped) = chars.next() {
                            part.push(escaped);
                        }
                    } else if quoted == character {
                        break;
                    }
                }
            }
            '{' | '[' | '(' => {
                let ending = match character {
                    '{' => '}',
                    '[' => ']',
                    _ => ')',
                };
                part.push(character);
                part.push_str(&debug_container(chars, Some(ending), character == '{'));
                part.push(ending);
            }
            ',' if close.is_some() => parts.push(std::mem::take(&mut part).trim().to_owned()),
            ending if Some(ending) == close => break,
            _ => part.push(character),
        }
    }
    if !part.is_empty() {
        parts.push(part.trim().to_owned());
    }
    if sort {
        parts.sort();
    }
    parts.join(",")
}

//! Decide whether match arms cover a variant's payload (#1741).
//!
//! [`super::match_`] owns which subjects are checked for exhaustiveness (enums, `Option` and `Result`) and reports each
//! missing variant. This module answers the question that walk used to skip: whether the arms that name a variant also
//! cover every value of its payload. `Some(0)` names `Some` but covers only one of its payloads, while `Ok(Some(x))`
//! beside `Ok(None)` covers `Ok` between them.
//!
//! The answer comes from the usual pattern-matrix walk. Each row holds the sub-patterns an arm still has to match, one
//! per column, and each column has a type. A column whose values the checker can enumerate (`bool`, `Option`, `Result`,
//! a source enum, a tuple, a model or class) is split into its constructors and each constructor's payload becomes new
//! columns; a column of an open scalar type (`int`, `str`, ...) matched by literals keeps only the rows that match any
//! value there. A column the checker cannot enumerate, or a head pattern it cannot place, is assumed covered, so the
//! walk never refuses a match the build would accept.

use std::collections::HashMap;

use crate::ast::{Literal, Pattern, PatternArg};
use crate::resolved_type_subst::{substitute_resolved_type, type_param_subst_map};
use crate::symbols::{FieldInfo, ResolvedType, TypeInfo};
use crate::typechecker::check_stmt::{TupleShape, classify_tuple_shape};
use incan_lang::lang::surface::constructors::{self, ConstructorId};

use super::TypeChecker;
use super::match_::borrowed_pattern_subject;

/// Deepest payload nesting the walk follows before it assumes the remaining columns covered.
const MAX_COVERAGE_DEPTH: usize = 32;

/// One row of the coverage matrix: the sub-pattern an arm still has to match in each column.
///
/// `None` is a wildcard the walk introduced for a payload the arm did not spell (a wildcard row specialized to a
/// constructor, or a payload position the pattern left out).
pub(super) type CoverageRow<'p> = Vec<Option<&'p Pattern>>;

/// How a head pattern names one constructor of a column's type.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CoverageKey {
    /// An enum, `Option` or `Result` variant, by every spelling a pattern may use (the canonical name first).
    Variant(Vec<String>),
    /// `Option`'s `None`, written as the `None` literal or the `None` constructor.
    OptionNone,
    /// One of the two `bool` literals.
    Bool(bool),
    /// The single constructor of a tuple.
    Tuple,
    /// The single constructor of a model or class, written with the type's name and named fields.
    Record(String),
}

/// One constructor of a column type together with the types of the payload columns it opens.
#[derive(Debug, Clone)]
pub(super) struct CoverageConstructor {
    key: CoverageKey,
    fields: Vec<ResolvedType>,
    /// Canonical field names of a record constructor in declaration order; empty for every other constructor.
    field_names: Vec<String>,
    /// Checked fields of a record constructor, used to resolve a pattern's field aliases.
    record_fields: HashMap<String, FieldInfo>,
}

impl CoverageConstructor {
    /// Build a constructor without a payload.
    fn unit(key: CoverageKey) -> Self {
        Self {
            key,
            fields: Vec::new(),
            field_names: Vec::new(),
            record_fields: HashMap::new(),
        }
    }

    /// Build a variant constructor from its canonical name, its other accepted spellings and its payload types.
    fn variant(name: &str, aliases: Vec<String>, fields: Vec<ResolvedType>) -> Self {
        let mut spellings = vec![name.to_string()];
        spellings.extend(aliases);
        Self {
            key: CoverageKey::Variant(spellings),
            fields,
            field_names: Vec::new(),
            record_fields: HashMap::new(),
        }
    }

    /// Return the types of the payload columns this constructor opens.
    pub(super) fn payload_types(&self) -> &[ResolvedType] {
        &self.fields
    }

    /// Spell the constructor the way a missing-pattern diagnostic names it.
    ///
    /// A variant no arm names is its bare name, as the diagnostic always spelled it. A variant that arms name without
    /// covering its whole payload is written with a wildcard per payload position (`Some(_)`), which is the arm that
    /// would complete the match.
    pub(super) fn missing_label(&self, partially_covered: bool) -> String {
        let name = match &self.key {
            CoverageKey::Variant(spellings) => spellings.first().cloned().unwrap_or_default(),
            CoverageKey::OptionNone => constructors::as_str(ConstructorId::None).to_string(),
            CoverageKey::Bool(value) => value.to_string(),
            CoverageKey::Tuple => String::new(),
            CoverageKey::Record(type_name) => type_name.clone(),
        };
        if !partially_covered || self.fields.is_empty() {
            return name;
        }
        let wildcards = vec!["_"; self.fields.len()].join(", ");
        format!("{name}({wildcards})")
    }
}

/// Return the pattern at the head of a row, `None` when the head is an introduced wildcard or the row is empty.
fn row_head<'p>(row: &CoverageRow<'p>) -> Option<&'p Pattern> {
    row.first().copied().flatten()
}

/// Whether a head pattern matches every value of its column.
fn coverage_head_is_wild(head: Option<&Pattern>) -> bool {
    matches!(head, None | Some(Pattern::Wildcard | Pattern::Binding(_)))
}

/// Whether a row's head matches every value of its column.
pub(super) fn coverage_row_head_is_wild(row: &CoverageRow<'_>) -> bool {
    coverage_head_is_wild(row_head(row))
}

/// Whether a column type has too many values to enumerate but is matched by literals, which never cover it alone.
fn coverage_column_is_open_scalar(column: &ResolvedType) -> bool {
    matches!(
        column,
        ResolvedType::Int
            | ResolvedType::Float
            | ResolvedType::Numeric(_)
            | ResolvedType::Str
            | ResolvedType::FrozenStr
            | ResolvedType::Bytes
            | ResolvedType::FrozenBytes
    )
}

/// Rewrite every row so its head is neither a group nor an alternation.
///
/// A grouped head is replaced by the pattern it groups, and a row whose head is `A | B` becomes one row per
/// alternative, which is what an alternation covers. Row order does not matter to coverage.
pub(super) fn expand_coverage_heads(rows: Vec<CoverageRow<'_>>) -> Vec<CoverageRow<'_>> {
    let mut expanded = Vec::with_capacity(rows.len());
    let mut pending = rows;
    while let Some(mut row) = pending.pop() {
        match row_head(&row) {
            Some(Pattern::Group(inner)) => {
                if let Some(slot) = row.first_mut() {
                    *slot = Some(&inner.node);
                }
                pending.push(row);
            }
            Some(Pattern::Or(alternatives)) => {
                for alternative in alternatives {
                    let mut alternative_row = row.clone();
                    if let Some(slot) = alternative_row.first_mut() {
                        *slot = Some(&alternative.node);
                    }
                    pending.push(alternative_row);
                }
            }
            _ => expanded.push(row),
        }
    }
    expanded
}

impl TypeChecker {
    /// Return the variant constructors of a match subject whose missing variants a refusal names.
    ///
    /// These are a source enum, generic or not, `Option` and `Result`. Any other subject (a scalar, a tuple, a model)
    /// is checked as one value and reported as missing `_`.
    pub(super) fn match_subject_variant_constructors(
        &self,
        subject_ty: &ResolvedType,
    ) -> Option<Vec<CoverageConstructor>> {
        let enforced = subject_ty.is_option()
            || subject_ty.is_result()
            || matches!(
                subject_ty,
                ResolvedType::Named(name) | ResolvedType::Generic(name, _)
                    if matches!(self.lookup_type_info(name), Some(TypeInfo::Enum(_)))
            );
        if !enforced {
            return None;
        }
        self.coverage_constructors(subject_ty)
    }

    /// Specialize every row to one constructor, keeping the rows whose head names it or matches anything.
    pub(super) fn specialize_coverage_rows<'p>(
        &self,
        rows: &[CoverageRow<'p>],
        constructor: &CoverageConstructor,
    ) -> Vec<CoverageRow<'p>> {
        rows.iter()
            .filter_map(|row| self.specialize_coverage_row(row, constructor))
            .collect()
    }

    /// Whether `rows` together match every value of the column types `columns`.
    ///
    /// With no columns left, any remaining row matches. Otherwise the head column is dropped when every head matches
    /// anything, split into its constructors when the checker can enumerate them, or reduced to the rows that match
    /// any value when it is an open scalar column matched by literals. When none of those applies (a type the checker
    /// cannot enumerate, a head it cannot place, a nesting deeper than `MAX_COVERAGE_DEPTH`) the rows are assumed to
    /// cover it: this check may only refuse what the build would refuse.
    pub(super) fn coverage_rows_exhaustive(
        &self,
        rows: Vec<CoverageRow<'_>>,
        columns: &[ResolvedType],
        depth: usize,
    ) -> bool {
        let Some((column, rest)) = columns.split_first() else {
            return !rows.is_empty();
        };
        if depth > MAX_COVERAGE_DEPTH {
            return true;
        }
        let rows = expand_coverage_heads(rows);

        // ---- Every head matches anything: the column decides nothing ----
        if rows.iter().all(coverage_row_head_is_wild) {
            let tails: Vec<CoverageRow<'_>> = rows.into_iter().map(|row| row.into_iter().skip(1).collect()).collect();
            return self.coverage_rows_exhaustive(tails, rest, depth + 1);
        }

        let (column, _) = borrowed_pattern_subject(column);
        let Some(column_constructors) = self.coverage_constructors(column) else {
            // ---- Open scalar column: literals never cover it, so only the rows that match anything remain ----
            let literal_heads_only = rows
                .iter()
                .all(|row| coverage_row_head_is_wild(row) || matches!(row_head(row), Some(Pattern::Literal(_))));
            if !coverage_column_is_open_scalar(column) || !literal_heads_only {
                return true;
            }
            let defaults: Vec<CoverageRow<'_>> = rows
                .into_iter()
                .filter(coverage_row_head_is_wild)
                .map(|row| row.into_iter().skip(1).collect())
                .collect();
            return self.coverage_rows_exhaustive(defaults, rest, depth + 1);
        };

        // ---- Enumerable column: every constructor's payload must be covered ----
        let every_head_is_placed = rows.iter().all(|row| {
            coverage_row_head_is_wild(row)
                || row_head(row).is_some_and(|head| {
                    column_constructors
                        .iter()
                        .any(|constructor| self.coverage_head_payload(head, constructor).is_some())
                })
        });
        if !every_head_is_placed {
            return true;
        }
        column_constructors.iter().all(|constructor| {
            let specialized = self.specialize_coverage_rows(&rows, constructor);
            let mut payload_columns = constructor.fields.clone();
            payload_columns.extend_from_slice(rest);
            self.coverage_rows_exhaustive(specialized, &payload_columns, depth + 1)
        })
    }

    /// Specialize one row to a constructor: its payload sub-patterns followed by the rest of the row.
    ///
    /// A head that matches anything contributes a wildcard per payload column; a head naming another constructor drops
    /// the row (`None`).
    fn specialize_coverage_row<'p>(
        &self,
        row: &CoverageRow<'p>,
        constructor: &CoverageConstructor,
    ) -> Option<CoverageRow<'p>> {
        let head = row_head(row);
        let mut specialized = if coverage_head_is_wild(head) {
            vec![None; constructor.fields.len()]
        } else {
            self.coverage_head_payload(head?, constructor)?
        };
        specialized.extend(row.iter().skip(1).copied());
        Some(specialized)
    }

    /// Return the payload sub-patterns a head pattern gives one constructor, padded with wildcards to the payload's
    /// width, or `None` when the head names a different constructor.
    ///
    /// A payload position the pattern does not spell (a named sub-pattern on a variant, a field a record pattern
    /// leaves out) is a wildcard, which is how the build reads it too.
    fn coverage_head_payload<'p>(
        &self,
        head: &'p Pattern,
        constructor: &CoverageConstructor,
    ) -> Option<CoverageRow<'p>> {
        let width = constructor.fields.len();
        match (&constructor.key, head) {
            (CoverageKey::Bool(expected), Pattern::Literal(Literal::Bool(value))) => {
                (*expected == *value).then(Vec::new)
            }
            (CoverageKey::OptionNone, Pattern::Literal(Literal::None)) => Some(Vec::new()),
            (CoverageKey::OptionNone, Pattern::Constructor(name, _)) => {
                let (_, variant) = Self::split_pattern_constructor_name(name.node.as_str());
                (constructors::from_str(variant) == Some(ConstructorId::None)).then(Vec::new)
            }
            (CoverageKey::Variant(spellings), Pattern::Constructor(name, args)) => {
                let (_, variant) = Self::split_pattern_constructor_name(name.node.as_str());
                if !spellings.iter().any(|spelling| spelling.as_str() == variant) {
                    return None;
                }
                let mut payload: CoverageRow<'p> = args
                    .iter()
                    .filter_map(|arg| match arg {
                        PatternArg::Positional(pattern) => Some(Some(&pattern.node)),
                        PatternArg::Named(_, _) => None,
                    })
                    .take(width)
                    .collect();
                payload.resize(width, None);
                Some(payload)
            }
            (CoverageKey::Tuple, Pattern::Tuple(items)) => {
                let mut payload: CoverageRow<'p> = items.iter().map(|item| Some(&item.node)).take(width).collect();
                payload.resize(width, None);
                Some(payload)
            }
            (CoverageKey::Record(type_name), Pattern::Constructor(name, args)) => {
                let (_, written) = Self::split_pattern_constructor_name(name.node.as_str());
                if written != type_name.as_str() {
                    return None;
                }
                let mut payload: CoverageRow<'p> = vec![None; width];
                for arg in args {
                    let PatternArg::Named(field, pattern) = arg else {
                        continue;
                    };
                    let Some((canonical, _)) =
                        self.resolve_field_info(&constructor.record_fields, &field.node, true, true)
                    else {
                        continue;
                    };
                    let slot = constructor
                        .field_names
                        .iter()
                        .position(|field_name| field_name.as_str() == canonical.as_str())
                        .and_then(|index| payload.get_mut(index));
                    if let Some(slot) = slot {
                        *slot = Some(&pattern.node);
                    }
                }
                Some(payload)
            }
            _ => None,
        }
    }

    /// Enumerate the constructors of a column type, or `None` when the checker cannot enumerate its values.
    ///
    /// `bool`, `Option`, `Result`, tuples, source enums, models and classes are enumerable; a source enum's and a
    /// record's payload types are instantiated with the column's type arguments. Everything else (scalars, unions,
    /// type parameters, Rust types, newtypes) is not.
    fn coverage_constructors(&self, column: &ResolvedType) -> Option<Vec<CoverageConstructor>> {
        if matches!(column, ResolvedType::Bool) {
            return Some(vec![
                CoverageConstructor::unit(CoverageKey::Bool(true)),
                CoverageConstructor::unit(CoverageKey::Bool(false)),
            ]);
        }
        if column.is_option() {
            let inner = column.option_inner_type().cloned().unwrap_or(ResolvedType::Unknown);
            return Some(vec![
                CoverageConstructor::variant(constructors::as_str(ConstructorId::Some), Vec::new(), vec![inner]),
                CoverageConstructor::unit(CoverageKey::OptionNone),
            ]);
        }
        if column.is_result() {
            let ok = column.result_ok_type().cloned().unwrap_or(ResolvedType::Unknown);
            let err = column.result_err_type().cloned().unwrap_or(ResolvedType::Unknown);
            return Some(vec![
                CoverageConstructor::variant(constructors::as_str(ConstructorId::Ok), Vec::new(), vec![ok]),
                CoverageConstructor::variant(constructors::as_str(ConstructorId::Err), Vec::new(), vec![err]),
            ]);
        }
        if let TupleShape::Tuple(items) = classify_tuple_shape(column) {
            return Some(vec![CoverageConstructor {
                key: CoverageKey::Tuple,
                fields: items,
                field_names: Vec::new(),
                record_fields: HashMap::new(),
            }]);
        }

        let (type_name, type_args): (&str, &[ResolvedType]) = match column {
            ResolvedType::Named(name) => (name.as_str(), &[]),
            ResolvedType::Generic(name, args) => (name.as_str(), args.as_slice()),
            _ => return None,
        };
        match self.lookup_type_info(type_name)? {
            TypeInfo::Enum(info) => {
                let substitution = type_param_subst_map(&info.type_params, type_args);
                let variants: Vec<CoverageConstructor> = info
                    .variants
                    .iter()
                    .map(|variant| {
                        let aliases: Vec<String> = info
                            .variant_aliases
                            .iter()
                            .filter(|(_, canonical)| canonical.as_str() == variant.as_str())
                            .map(|(alias, _)| alias.clone())
                            .collect();
                        let fields: Vec<ResolvedType> = info
                            .variant_fields
                            .get(variant)
                            .map(|fields| {
                                fields
                                    .iter()
                                    .map(|field| substitute_resolved_type(field, &substitution))
                                    .collect()
                            })
                            .unwrap_or_default();
                        CoverageConstructor::variant(variant, aliases, fields)
                    })
                    .collect();
                Some(variants)
            }
            TypeInfo::Model(info) => Some(vec![record_constructor(
                type_name,
                &info.fields,
                &info.field_order,
                &info.type_params,
                type_args,
            )]),
            TypeInfo::Class(info) => Some(vec![record_constructor(
                type_name,
                &info.fields,
                &info.field_order,
                &info.type_params,
                type_args,
            )]),
            _ => None,
        }
    }
}

/// Build the single constructor of a model or class column, its payload being the fields in declaration order.
fn record_constructor(
    type_name: &str,
    fields: &HashMap<String, FieldInfo>,
    field_order: &[String],
    type_params: &[String],
    type_args: &[ResolvedType],
) -> CoverageConstructor {
    let substitution = type_param_subst_map(type_params, type_args);
    let field_types = field_order
        .iter()
        .map(|name| {
            fields
                .get(name)
                .map(|field| substitute_resolved_type(&field.ty, &substitution))
                .unwrap_or(ResolvedType::Unknown)
        })
        .collect();
    CoverageConstructor {
        key: CoverageKey::Record(type_name.to_string()),
        fields: field_types,
        field_names: field_order.to_vec(),
        record_fields: fields.clone(),
    }
}

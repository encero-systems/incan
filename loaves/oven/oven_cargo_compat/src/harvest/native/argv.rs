//! Declarative classification of captured native compiler arguments.

use std::path::Path;

/// Semantic value produced by one native compiler argument form.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum TypedArgument<'a> {
    /// The compile source consumed by -c.
    Source(&'a str),
    /// The object output consumed by -o.
    Output(&'a str),
    /// A path-bearing flag and its physical value.
    Path {
        /// Canonical separate spelling emitted for the flag, when one exists.
        flag: Option<&'static str>,
        /// Physical path captured from the compiler invocation.
        value: &'a str,
    },
    /// An argument whose bytes carry no path semantics.
    Literal(&'a str),
}

/// One classified argument plus the number of captured argv entries it consumes.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct ClassifiedArgument<'a> {
    /// Typed meaning used by native-link adoption.
    pub(super) argument: TypedArgument<'a>,
    /// One for joined/fallback forms and two for separate-value forms.
    pub(super) consumed: usize,
}

/// Meaning assigned to the value of a flag form.
#[derive(Clone, Copy)]
enum ValueKind {
    Source,
    Output,
}

/// Declarative compiler-argument forms, checked in table order.
#[derive(Clone, Copy)]
enum ArgumentForm {
    SeparateValue {
        spellings: &'static [&'static str],
        kind: ValueKind,
    },
    JoinedValue {
        prefix: &'static str,
        canonical: &'static str,
    },
    EqualsValue {
        spellings: &'static [&'static str],
    },
    PathPrefix {
        spellings: &'static [&'static str],
    },
    AbsolutePath,
    Literal,
}

/// The complete native compiler argv grammar understood by harvest adoption.
///
/// Table order is semantic: exact separate forms precede joined prefixes, and the two fallbacks remain last.
const ARGUMENT_FORMS: &[ArgumentForm] = &[
    ArgumentForm::SeparateValue {
        spellings: &["-c"],
        kind: ValueKind::Source,
    },
    ArgumentForm::SeparateValue {
        spellings: &["-o"],
        kind: ValueKind::Output,
    },
    ArgumentForm::PathPrefix {
        spellings: &["-I", "-isysroot", "--sysroot", "-resource-dir"],
    },
    ArgumentForm::EqualsValue {
        spellings: &["--sysroot", "-resource-dir"],
    },
    ArgumentForm::JoinedValue {
        prefix: "-I",
        canonical: "-I",
    },
    ArgumentForm::AbsolutePath,
    ArgumentForm::Literal,
];

/// Classify one compiler argument using the declarative form table.
pub(super) fn classify_argument<'a>(arguments: &'a [String], index: usize) -> Result<ClassifiedArgument<'a>, String> {
    let argument = arguments
        .get(index)
        .ok_or_else(|| "compiler argument index is out of bounds".to_string())?;
    for form in ARGUMENT_FORMS {
        match *form {
            ArgumentForm::SeparateValue { spellings, kind } if spellings.contains(&argument.as_str()) => {
                let value = arguments
                    .get(index + 1)
                    .ok_or_else(|| format!("{argument} has no value"))?;
                let argument = match kind {
                    ValueKind::Source => TypedArgument::Source(value),
                    ValueKind::Output => TypedArgument::Output(value),
                };
                return Ok(ClassifiedArgument { argument, consumed: 2 });
            }
            ArgumentForm::PathPrefix { spellings } if spellings.contains(&argument.as_str()) => {
                let value = arguments
                    .get(index + 1)
                    .ok_or_else(|| format!("{argument} has no value"))?;
                return Ok(ClassifiedArgument {
                    argument: TypedArgument::Path {
                        flag: spellings.iter().copied().find(|spelling| *spelling == argument),
                        value,
                    },
                    consumed: 2,
                });
            }
            ArgumentForm::EqualsValue { spellings } => {
                if let Some((flag, value)) = spellings.iter().find_map(|flag| {
                    argument
                        .strip_prefix(flag)
                        .and_then(|rest| rest.strip_prefix('='))
                        .map(|value| (*flag, value))
                }) {
                    return Ok(ClassifiedArgument {
                        argument: TypedArgument::Path {
                            flag: Some(flag),
                            value,
                        },
                        consumed: 1,
                    });
                }
            }
            ArgumentForm::JoinedValue { prefix, canonical } => {
                if let Some(value) = argument.strip_prefix(prefix).filter(|value| !value.is_empty()) {
                    return Ok(ClassifiedArgument {
                        argument: TypedArgument::Path {
                            flag: Some(canonical),
                            value,
                        },
                        consumed: 1,
                    });
                }
            }
            ArgumentForm::AbsolutePath if Path::new(argument).is_absolute() => {
                return Ok(ClassifiedArgument {
                    argument: TypedArgument::Path {
                        flag: None,
                        value: argument,
                    },
                    consumed: 1,
                });
            }
            ArgumentForm::Literal => {
                return Ok(ClassifiedArgument {
                    argument: TypedArgument::Literal(argument),
                    consumed: 1,
                });
            }
            ArgumentForm::SeparateValue { .. } | ArgumentForm::PathPrefix { .. } | ArgumentForm::AbsolutePath => {}
        }
    }
    Err("compiler argument did not match the complete form table".to_string())
}

#[cfg(test)]
mod tests {
    use super::{ClassifiedArgument, TypedArgument, classify_argument};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Separate-value forms consume the flag and its typed value.
    #[test]
    fn separate_value_form() -> TestResult {
        let arguments = vec!["-c".to_string(), "source.c".to_string()];
        assert_eq!(
            classify_argument(&arguments, 0)?,
            ClassifiedArgument {
                argument: TypedArgument::Source("source.c"),
                consumed: 2,
            }
        );
        Ok(())
    }

    /// Joined-value forms split their canonical flag from the path.
    #[test]
    fn joined_value_form() -> TestResult {
        let arguments = vec!["-Iinclude".to_string()];
        assert_eq!(
            classify_argument(&arguments, 0)?,
            ClassifiedArgument {
                argument: TypedArgument::Path {
                    flag: Some("-I"),
                    value: "include",
                },
                consumed: 1,
            }
        );
        Ok(())
    }

    /// Equals-value forms normalize to a separate canonical flag and path.
    #[test]
    fn equals_value_form() -> TestResult {
        let arguments = vec!["--sysroot=/sdk".to_string()];
        assert_eq!(
            classify_argument(&arguments, 0)?,
            ClassifiedArgument {
                argument: TypedArgument::Path {
                    flag: Some("--sysroot"),
                    value: "/sdk",
                },
                consumed: 1,
            }
        );
        Ok(())
    }

    /// Path-prefix forms consume their following path.
    #[test]
    fn path_prefix_form() -> TestResult {
        let arguments = vec!["-I".to_string(), "include".to_string()];
        assert_eq!(
            classify_argument(&arguments, 0)?,
            ClassifiedArgument {
                argument: TypedArgument::Path {
                    flag: Some("-I"),
                    value: "include",
                },
                consumed: 2,
            }
        );
        Ok(())
    }

    /// Absolute paths carry path semantics without an emitted flag.
    #[test]
    fn absolute_path_form() -> TestResult {
        let arguments = vec!["/sdk/include".to_string()];
        assert_eq!(
            classify_argument(&arguments, 0)?,
            ClassifiedArgument {
                argument: TypedArgument::Path {
                    flag: None,
                    value: "/sdk/include",
                },
                consumed: 1,
            }
        );
        Ok(())
    }

    /// Ordinary compiler switches remain byte-identical literals.
    #[test]
    fn literal_form() -> TestResult {
        let arguments = vec!["-Wall".to_string()];
        assert_eq!(
            classify_argument(&arguments, 0)?,
            ClassifiedArgument {
                argument: TypedArgument::Literal("-Wall"),
                consumed: 1,
            }
        );
        Ok(())
    }
}

//! Import-aware reductions of every real `internal_models` stop exposed by the release-standard relink.

use super::*;

const PROVIDER: &str = "@derive(Eq)\npub enum Family:\n    Declaration\n    Local\n    Final\n@derive(Eq)\npub model Description:\n    pub profile: str\n@derive(Eq)\npub model RowRecord:\n    pub value: str\n@derive(Eq)\npub model Field:\n    pub family: str\n    pub node: str\n    pub selector: str\n    pub values: list[str]\n@derive(Eq)\npub model Record:\n    pub profile: str\n    pub owner: str\n    pub root: str\n    pub family: Family\n    pub fields: list[Field]\n@derive(Eq)\npub model Outputs:\n    pub namespaces: list[str]\n    pub values: list[str]\n    pub fallbacks: list[str]\npub enum Logical:\n    Text(str)\npub class sha256:\n    @staticmethod\n    pub def digest(value: bytes) -> bytes:\n        return value\npub def encode_rows(description: Description, rows: list[RowRecord], maximum: int) -> Result[bytes, str]:\n    return Ok(b\"row\")\npub def decode_rows(value: bytes, descriptions: list[Description], maximum: int) -> Result[list[str], str]:\n    return Ok([])\npub def operation_budget() -> Result[int, str]:\n    return Ok(1)\n";

/// Require selected same-Loaf dependency bodies through both source collection and the sealed interface route.
fn require_module_bodies(consumer: &str, names: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    let mut checkers = super::one_type::consumer_checkers(PROVIDER, consumer)?;
    checkers.extend(super::one_type::package_consumer_checkers(PROVIDER, consumer)?);
    for (route, checker) in checkers {
        for name in names {
            let flow = checker
                .type_info
                .protocols
                .explicit_input_flow
                .iter()
                .find(|(owner, _)| owner.declaration_name == *name)
                .map(|(_, flow)| flow)
                .ok_or_else(|| format!("{route}: missing {name} flow; errors={:?}", checker.errors))?;
            if !matches!(flow, InputFlow::Checked { .. }) {
                return Err(format!("{route}: {name} did not retain a checked graph: {flow:?}").into());
            }
        }
        assert!(checker.errors.is_empty(), "{route}: {:?}", checker.errors);
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum PersistedUse<'a> {
    CallResult(&'a str),
    Move { source: &'a str, root: &'a str },
}

/// Require the retained source graph to persist exact ValueUse authority and successfully authenticate it again at
/// the executable-function boundary on collected, sealed and Package dependency routes.
fn require_replayed_body(
    consumer: &str,
    name: &str,
    expected: &[PersistedUse<'_>],
    map_err: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let tokens = crate::lexer::lex(consumer).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let function = program
        .declarations
        .iter()
        .find_map(|declaration| match &declaration.node {
            crate::ast::Declaration::Function(function) if function.name == name => Some(function),
            _ => None,
        })
        .ok_or_else(|| format!("missing {name} function"))?;
    let mut checkers = super::one_type::consumer_checkers(PROVIDER, consumer)?;
    checkers.extend(super::one_type::package_consumer_checkers(PROVIDER, consumer)?);
    for (route, checker) in checkers {
        let (owner, flow) = checker
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .find(|(owner, _)| owner.declaration_name == name)
            .ok_or_else(|| format!("{route}: missing {name} flow"))?;
        let InputFlow::Checked { graph, .. } = flow else {
            return Err(format!("{route}: {name} did not retain a checked graph: {flow:?}").into());
        };
        for expected in expected {
            let retained = graph.value_uses.iter().any(|(span, value_use)| {
                let source = consumer.get(span.start..span.end);
                match expected {
                    PersistedUse::CallResult(expected_source) => {
                        value_use.kind == flow::ValueUseKind::CallResult && source == Some(*expected_source)
                    }
                    PersistedUse::Move {
                        source: expected_source,
                        root,
                    } => {
                        value_use.kind == flow::ValueUseKind::Move
                            && source == Some(*expected_source)
                            && value_use.accesses.iter().any(|access| {
                                matches!(&access.place.root,
                                    flow::Root::Binding(identity) if identity.declaration_name == *root)
                            })
                    }
                }
            });
            if !retained {
                return Err(format!("{route}: {name} did not retain the expected classified value use").into());
            }
        }
        if map_err
            && !graph
                .value_uses
                .values()
                .any(|value_use| value_use.result_map_err.is_some())
        {
            return Err(format!("{route}: {name} did not retain its exact map_err classification").into());
        }
        if !matches!(
            checker.type_info.executable_function_input(owner, function),
            Ok(Some(_))
        ) {
            return Err(format!("{route}: {name} checked graph did not replay at the executable boundary").into());
        }
        if !checker.errors.is_empty() {
            return Err(format!("{route}: {name} produced diagnostics: {:?}", checker.errors).into());
        }
    }
    Ok(())
}

fn require_package_body(consumer: &str, name: &str) -> Result<(), Box<dyn std::error::Error>> {
    for (route, checker) in super::one_type::package_consumer_checkers(PROVIDER, consumer)? {
        let flow = checker
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .find(|(owner, _)| owner.declaration_name == name)
            .map(|(_, flow)| flow)
            .ok_or_else(|| format!("{route}: missing {name} flow; errors={:?}", checker.errors))?;
        if !matches!(flow, InputFlow::Checked { .. }) {
            return Err(format!("{route}: {name} did not retain a checked graph: {flow:?}").into());
        }
        assert!(checker.errors.is_empty(), "{route}: {:?}", checker.errors);
    }
    Ok(())
}

fn require_module_refusal(consumer: &str, name: &str, stop: &str) -> Result<(), Box<dyn std::error::Error>> {
    for (route, checker) in super::one_type::consumer_checkers(PROVIDER, consumer)? {
        let flow = checker
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .find(|(owner, _)| owner.declaration_name == name)
            .map(|(_, flow)| flow)
            .ok_or_else(|| format!("{route}: missing refused {name} flow; errors={:?}", checker.errors))?;
        let InputFlow::Unavailable(unavailable) = flow else {
            return Err(format!("{route}: {name} unexpectedly entered the checked graph: {flow:?}").into());
        };
        let source_stop = consumer
            .get(unavailable.span.start..unavailable.span.end)
            .ok_or_else(|| format!("{route}: {name} refusal span is outside the consumer source"))?;
        if !source_stop.contains(stop) {
            return Err(format!("{route}: {name} stopped at {source_stop:?}, expected {stop:?}").into());
        }
    }
    Ok(())
}

fn require_package_refusal(consumer: &str, name: &str, stop: &str) -> Result<(), Box<dyn std::error::Error>> {
    for (route, checker) in super::one_type::package_consumer_checkers(PROVIDER, consumer)? {
        let flow = checker
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .find(|(owner, _)| owner.declaration_name == name)
            .map(|(_, flow)| flow)
            .ok_or_else(|| format!("{route}: missing refused {name} flow; errors={:?}", checker.errors))?;
        let InputFlow::Unavailable(unavailable) = flow else {
            return Err(format!("{route}: {name} unexpectedly entered the checked graph: {flow:?}").into());
        };
        let source_stop = consumer
            .get(unavailable.span.start..unavailable.span.end)
            .ok_or_else(|| format!("{route}: {name} refusal span is outside the consumer source"))?;
        if !source_stop.contains(stop) {
            return Err(format!("{route}: {name} stopped at {source_stop:?}, expected {stop:?}").into());
        }
    }
    Ok(())
}

fn require_module_diagnostic_at(consumer: &str, code: &str, stop: &str) -> Result<(), Box<dyn std::error::Error>> {
    for (route, checker) in super::one_type::consumer_checkers(PROVIDER, consumer)? {
        if checker.errors.is_empty() || checker.errors.iter().any(|error| error.stable_code() != Some(code)) {
            return Err(format!("{route}: expected only {code} diagnostics, got {:?}", checker.errors).into());
        }
        let at_stop = checker.errors.iter().any(|error| {
            error.stable_code() == Some(code)
                && consumer
                    .get(error.span.start..error.span.end)
                    .is_some_and(|source| source.contains(stop))
        });
        if !at_stop {
            return Err(format!("{route}: {code} did not point at {stop:?}: {:?}", checker.errors).into());
        }
    }
    Ok(())
}

#[test]
fn real_source_encode_map_err_accepts_fresh_description_row_and_const_arguments()
-> Result<(), Box<dyn std::error::Error>> {
    let encode = "from views import Description, Record, RowRecord, encode_rows\n\nconst MAX_BYTES: int = 4096\nenum Fault:\n    Bad\ndef checker_description() -> Description:\n    return Description(profile=\"profile\")\ndef record_value(record: Borrowed[Record]) -> Result[str, Fault]:\n    return Ok(\"logical\")\ndef encode_record(record: Borrowed[Record]) -> Result[bytes, Fault]:\n    logical = record_value(record)?\n    return encode_rows(checker_description(), [RowRecord(value=logical)], MAX_BYTES).map_err((_) => Fault.Bad)\n";
    require_replayed_body(
        encode,
        "encode_record",
        &[
            PersistedUse::CallResult("checker_description()"),
            PersistedUse::Move {
                source: "logical",
                root: "logical",
            },
        ],
        true,
    )
}

#[test]
fn real_source_constructor_local_move_keeps_post_move_use_closed() -> Result<(), Box<dyn std::error::Error>> {
    let post_move = "from views import Description, Record, RowRecord, encode_rows\n\nconst MAX_BYTES: int = 4096\nenum Fault:\n    Bad\ndef checker_description() -> Description:\n    return Description(profile=\"profile\")\ndef record_value(record: Borrowed[Record]) -> Result[str, Fault]:\n    return Ok(\"logical\")\ndef encode_record(record: Borrowed[Record]) -> Result[bytes, Fault]:\n    logical = record_value(record)?\n    return encode_rows(checker_description(), [RowRecord(value=logical)], MAX_BYTES + len(logical)).map_err((_) => Fault.Bad)\n";
    require_module_diagnostic_at(post_move, "INCAN-T0126", "logical")
}

#[test]
fn real_source_budget_map_err_accepts_zero_argument_result() -> Result<(), Box<dyn std::error::Error>> {
    let budget = "from views import Record, operation_budget\n\nenum Fault:\n    Bad\ndef observe(record: Borrowed[Record]) -> int:\n    return len(record.owner)\ndef semantic_digest(record: Record) -> Result[int, Fault]:\n    mut budget = operation_budget().map_err((_) => Fault.Bad)?\n    return Ok(budget + observe(record))\n";
    require_module_bodies(budget, &["semantic_digest"])
}

#[test]
fn real_source_decode_map_err_accepts_fresh_description_list_and_const_arguments()
-> Result<(), Box<dyn std::error::Error>> {
    let decode = "from views import Description, Family, Record, decode_rows\n\nconst MAX_BYTES: int = 4096\nenum Fault:\n    Bad\ndef checker_description() -> Description:\n    return Description(profile=\"profile\")\ndef validate(record: Borrowed[Record]) -> Result[None, Fault]:\n    return Ok(None)\ndef decoded_record(encoded: bytes) -> Result[int, Fault]:\n    rows = decode_rows(encoded, [checker_description()], MAX_BYTES).map_err((_) => Fault.Bad)?\n    record = Record(profile=\"profile\", owner=\"owner\", root=\"root\", family=Family.Local, fields=[])\n    validate(record)?\n    return Ok(len(rows))\n";
    require_replayed_body(
        decode,
        "decoded_record",
        &[PersistedUse::Move {
            source: "encoded",
            root: "encoded",
        }],
        true,
    )
}

#[test]
fn real_source_decode_owned_argument_keeps_move_boundaries_closed() -> Result<(), Box<dyn std::error::Error>> {
    let post_move = "from views import Description, Family, Record, decode_rows\n\nconst MAX_BYTES: int = 4096\nenum Fault:\n    Bad\ndef checker_description() -> Description:\n    return Description(profile=\"profile\")\ndef validate(record: Borrowed[Record]) -> Result[None, Fault]:\n    return Ok(None)\ndef decoded_record(encoded: bytes) -> Result[int, Fault]:\n    rows = decode_rows(encoded, [checker_description()], MAX_BYTES).map_err((_) => Fault.Bad)?\n    record = Record(profile=\"profile\", owner=\"owner\", root=\"root\", family=Family.Local, fields=[])\n    validate(record)?\n    return Ok(len(rows) + len(encoded))\n";
    require_module_diagnostic_at(post_move, "INCAN-T0126", "encoded")?;
    let borrowed = "from views import Description, Family, Record, decode_rows\n\nconst MAX_BYTES: int = 4096\nenum Fault:\n    Bad\ndef checker_description() -> Description:\n    return Description(profile=\"profile\")\ndef validate(record: Borrowed[Record]) -> Result[None, Fault]:\n    return Ok(None)\ndef decoded_record(encoded: Borrowed[bytes]) -> Result[int, Fault]:\n    rows = decode_rows(encoded, [checker_description()], MAX_BYTES).map_err((_) => Fault.Bad)?\n    record = Record(profile=\"profile\", owner=\"owner\", root=\"root\", family=Family.Local, fields=[])\n    validate(record)?\n    return Ok(len(rows))\n";
    require_module_diagnostic_at(borrowed, "INCAN-T0123", "encoded")?;
    let returned_view = "from views import Description, Family, Record, decode_rows\n\nconst MAX_BYTES: int = 4096\nenum Fault:\n    Bad\ndef checker_description() -> Description:\n    return Description(profile=\"profile\")\ndef view(encoded: Borrowed[bytes]) -> Borrowed[bytes] from encoded:\n    return encoded\ndef validate(record: Borrowed[Record]) -> Result[None, Fault]:\n    return Ok(None)\ndef decoded_record(encoded: Borrowed[bytes]) -> Result[int, Fault]:\n    rows = decode_rows(view(encoded), [checker_description()], MAX_BYTES).map_err((_) => Fault.Bad)?\n    record = Record(profile=\"profile\", owner=\"owner\", root=\"root\", family=Family.Local, fields=[])\n    validate(record)?\n    return Ok(len(rows))\n";
    require_module_diagnostic_at(returned_view, "INCAN-T0123", "view(encoded)")
}

#[test]
fn real_source_try_call_is_an_ordinary_argument_call_result() -> Result<(), Box<dyn std::error::Error>> {
    let consumer = "from views import Record, sha256\n\nenum Fault:\n    Bad\ndef encode_record(record: Borrowed[Record]) -> Result[bytes, Fault]:\n    return Ok(b\"encoded\")\ndef content_digest(record: Record) -> Result[bytes, Fault]:\n    return Ok(sha256.digest(encode_record(record)?))\n";
    require_replayed_body(
        consumer,
        "content_digest",
        &[PersistedUse::CallResult("encode_record(record)?")],
        false,
    )
}

#[test]
fn direct_selected_call_uses_one_persisted_call_result_authority() -> Result<(), Box<dyn std::error::Error>> {
    let consumer = "def produce() -> str:\n    return \"value\"\ndef observe(value: str) -> int:\n    return len(value)\ndef use(seed: Borrowed[int]) -> int:\n    return observe(produce())\n";
    require_replayed_body(consumer, "use", &[PersistedUse::CallResult("produce()")], false)
}

/// A constructor may move an exact imported enum Parameter once. Other imported nominals, borrowed values and field
/// projections cannot borrow that authority, and reusing the enum remains a graph conflict.
#[test]
fn imported_enum_parameter_constructor_move_is_exact_and_single_use() -> Result<(), Box<dyn std::error::Error>> {
    let accepted = "from views import Family\n\nmodel Field:\n    family: Family\ndef collect(seed: Borrowed[int], family: Family) -> Field:\n    return Field(family=family)\n";
    require_module_bodies(accepted, &["collect"])?;

    let non_enum = "from views import Record\n\nmodel Envelope:\n    record: Record\ndef collect(seed: Borrowed[int], record: Record) -> Envelope:\n    return Envelope(record=record)\n";
    require_module_diagnostic_at(non_enum, "INCAN-T0123", "record")?;
    let borrowed = "from views import Family\n\nmodel Field:\n    family: Family\ndef collect(seed: Borrowed[int], family: Borrowed[Family]) -> Field:\n    return Field(family=family)\n";
    require_module_diagnostic_at(borrowed, "INCAN-T0123", "family")?;
    let projected = "from views import Family\n\nmodel Holder:\n    family: Family\nmodel Field:\n    family: Family\ndef collect(seed: Borrowed[int], holder: Owned[Holder]) -> Field:\n    return Field(family=holder.family)\n";
    require_module_diagnostic_at(projected, "INCAN-T0123", "holder.family")?;

    let reused = "from views import Family\n\nmodel Field:\n    family: Family\ndef collect(seed: Borrowed[int], family: Family) -> Field:\n    first = Field(family=family)\n    second = Field(family=family)\n    return second\n";
    for (route, checker) in super::one_type::consumer_checkers(PROVIDER, reused)? {
        assert!(
            !checker.errors.is_empty()
                && checker
                    .errors
                    .iter()
                    .all(|error| error.stable_code() == Some("INCAN-T0126")),
            "{route}: imported enum reuse was not a move conflict: {:?}",
            checker.errors
        );
    }
    Ok(())
}

#[test]
fn same_leaf_different_origins_refuse_call_result_and_fresh_model_admission() -> Result<(), Box<dyn std::error::Error>>
{
    let provider = "@derive(Eq)\npub model Description:\n    pub profile: str\npub def observe(value: Description) -> int:\n    return len(value.profile)\npub def observe_many(values: list[Description]) -> int:\n    return len(values)\n";
    let consumer = "from views import Description, observe, observe_many\n\ndef fresh() -> Description:\n    return Description(profile=\"profile\")\ndef direct(seed: Borrowed[int]) -> int:\n    return observe(fresh())\ndef listed(seed: Borrowed[int]) -> int:\n    return observe_many([fresh()])\n";
    let mut checkers = super::one_type::consumer_checkers(provider, consumer)?;
    checkers.extend(super::one_type::package_consumer_checkers(provider, consumer)?);
    for (route, mut checker) in checkers {
        let retained = checker
            .type_info
            .declarations
            .named_type_identities
            .values()
            .find(|identity| identity.declaration_name == "Description")
            .cloned()
            .ok_or_else(|| format!("{route}: missing retained Description identity"))?;
        checker
            .type_info
            .declarations
            .named_type_identities
            .remove("Description");
        let mut conflicting = retained;
        conflicting.origin = incan_semantics_core::SymbolOrigin::Module(vec!["ambiguous".into()]);
        checker
            .type_info
            .declarations
            .named_type_identities
            .insert("__ambiguous_description_control".into(), conflicting);
        assert!(
            checker.type_info.checked_nominal_identity("Description").is_none(),
            "{route}: ambiguous same-leaf identities acquired published nominal authority"
        );
    }
    Ok(())
}

#[test]
fn real_source_imported_plain_enum_is_an_ordinary_observation() -> Result<(), Box<dyn std::error::Error>> {
    let consumer = "from views import Family\n\nmodel ObservedFamily:\n    name: str\n    family: Family\ndef family_name(family: Family) -> str:\n    match family:\n        Family.Declaration => return \"declaration\"\n        Family.Local => return \"local\"\n        Family.Final => return \"final\"\ndef from_family(seed: Borrowed[int], family: Family) -> ObservedFamily:\n    name = family_name(family)\n    return ObservedFamily(name=name, family=family)\n";
    let mut checkers = super::one_type::consumer_checkers(PROVIDER, consumer)?;
    checkers.extend(super::one_type::package_consumer_checkers(PROVIDER, consumer)?);
    for (route, checker) in checkers {
        let flow = checker
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .find(|(owner, _)| owner.declaration_name == "from_family")
            .map(|(_, flow)| flow)
            .ok_or_else(|| format!("{route}: missing from_family flow"))?;
        let InputFlow::Checked { graph, analysis } = flow else {
            return Err(format!("{route}: imported enum observation did not check: {flow:?}").into());
        };
        assert!(analysis.conflicts.is_empty(), "{route}: {:?}", analysis.conflicts);
        let observed = graph
            .invocations
            .values()
            .find(|invocation| invocation.callee.declaration_name == "family_name")
            .and_then(|invocation| invocation.arguments.first())
            .ok_or_else(|| format!("{route}: missing family_name argument"))?;
        assert!(observed.closed_enum_parameter_observer, "{route}");
        assert_eq!(
            observed.access,
            incan_semantics_core::IncanInputAccess::Inferred,
            "{route}"
        );
        assert!(
            !graph.value_uses.contains_key(&observed.span),
            "{route}: enum observation acquired Move authority"
        );
        let moves = graph
            .value_uses
            .iter()
            .filter(|(span, value_use)| {
                value_use.kind == flow::ValueUseKind::Move
                    && span.start > observed.span.start
                    && consumer.get(span.start..span.end) == Some("family")
            })
            .collect::<Vec<_>>();
        assert_eq!(moves.len(), 1, "{route}: final enum constructor Move is not unique");
        assert!(
            moves[0].1.accesses.iter().any(|access| {
                matches!(&access.place.root,
                    flow::Root::Binding(identity)
                        if identity.kind == incan_semantics_core::SemanticSourceTargetKind::Parameter
                            && identity.declaration_name == "family")
            }),
            "{route}: final enum Move retained the wrong root"
        );
        assert!(checker.errors.is_empty(), "{route}: {:?}", checker.errors);
    }

    let mutable = "from views import Family\n\ndef family_name(mut family: Family) -> str:\n    return \"family\"\ndef from_family(seed: Borrowed[int], family: Family) -> str:\n    return family_name(family)\n";
    require_module_refusal(mutable, "from_family", "family")
}

#[test]
fn real_source_public_and_private_constants_are_immutable_operands() -> Result<(), Box<dyn std::error::Error>> {
    let public_constant = "from views import Record\n\npub const PROFILE: str = \"profile\"\nenum Fault:\n    Bad\ndef validate(record: Borrowed[Record]) -> Result[None, Fault]:\n    if record.profile != PROFILE:\n        return Err(Fault.Bad)\n    return Ok(None)\n";
    require_module_bodies(public_constant, &["validate"])?;
    let private_constant = "from views import Outputs\n\nconst MAX_FIELDS: int = 4096\nenum Fault:\n    Bad\ndef encode_outputs(outputs: Borrowed[Outputs]) -> Result[int, Fault]:\n    output_count = len(outputs.namespaces) + len(outputs.values) + len(outputs.fallbacks)\n    if output_count > MAX_FIELDS:\n        return Err(Fault.Bad)\n    return Ok(output_count)\n";
    require_module_bodies(private_constant, &["encode_outputs"])
}

#[test]
fn real_source_package_enum_list_compound_materializes_elements() -> Result<(), Box<dyn std::error::Error>> {
    let consumer = "from views import Logical, Record\n\nenum Fault:\n    Bad\ndef record_value(record: Borrowed[Record]) -> Result[list[Logical], Fault]:\n    mut flattened: list[Logical] = []\n    for field in record.fields:\n        flattened += [Logical.Text(field.family), Logical.Text(field.node)]\n    return Ok(flattened)\n";
    require_package_body(consumer, "record_value")
}

#[test]
fn real_source_builtin_slice_is_a_fresh_iteration_producer() -> Result<(), Box<dyn std::error::Error>> {
    let consumer = "from views import Record\n\ndef from_native_fields(record: Borrowed[Record], rows: list[list[str]]) -> int:\n    mut count = 0\n    for row in rows[1:]:\n        count += len(row)\n    return count\n";
    require_module_bodies(consumer, &["from_native_fields"])
}

#[test]
fn real_source_retained_owner_fields_materialize_full_header() -> Result<(), Box<dyn std::error::Error>> {
    let consumer = "from views import Family, Record\n\nenum Fault:\n    Bad\ndef family_name(family: Family) -> str:\n    match family:\n        Family.Declaration => return \"declaration\"\n        Family.Local => return \"local\"\n        Family.Final => return \"final\"\ndef validate(record: Borrowed[Record]) -> Result[None, Fault]:\n    return Ok(None)\ndef native_fields(record: Record) -> Result[list[list[str]], Fault]:\n    validate(record)?\n    rows = [[\"header\", record.owner, record.root, family_name(record.family), record.profile]]\n    return Ok(rows)\n";
    require_module_bodies(consumer, &["native_fields"])
}

#[test]
fn real_source_routes_keep_capability_boundaries_closed() -> Result<(), Box<dyn std::error::Error>> {
    let captureful = "from views import Description, Record, RowRecord, encode_rows\n\nconst MAX_BYTES: int = 4096\nenum Fault:\n    Bad(str)\ndef checker_description() -> Description:\n    return Description(profile=\"profile\")\ndef encode_record(record: Borrowed[Record]) -> Result[bytes, Fault]:\n    owner = record.owner\n    return encode_rows(checker_description(), [RowRecord(value=record.owner)], MAX_BYTES).map_err((_) => Fault.Bad(owner))\n";
    require_module_refusal(captureful, "encode_record", "encode_rows")?;
    let mutable_static =
        "static LIMIT: int = 4096\ndef validate(value: Borrowed[int]) -> bool:\n    return value > LIMIT\n";
    require_module_refusal(mutable_static, "validate", "LIMIT")?;
    let whole_model =
        "from views import Record\n\ndef collect(record: Borrowed[Record]) -> list[Record]:\n    return [record]\n";
    require_module_refusal(whole_model, "collect", "record")?;
    let stored_result = "from views import Record, sha256\n\nenum Fault:\n    Bad\ndef encode_record(record: Borrowed[Record]) -> Result[bytes, Fault]:\n    return Ok(b\"encoded\")\ndef content_digest(record: Borrowed[Record]) -> Result[bytes, Fault]:\n    encoded = encode_record(record)\n    return Ok(sha256.digest(encoded?))\n";
    require_package_refusal(stored_result, "content_digest", "encoded?")?;
    let model_argument = "from views import Record\n\ndef consume(record: Record) -> int:\n    return len(record.owner)\ndef inspect(record: Borrowed[Record]) -> int:\n    return consume(record)\n";
    require_module_refusal(model_argument, "inspect", "record")?;
    let user_slice = "class Shelf:\n    pub rows: list[list[str]]\n    def __getslice__(self, start: Option[int], end: Option[int], step: Option[int]) -> list[list[str]]:\n        return self.rows\ndef collect(shelf: Borrowed[Shelf]) -> int:\n    mut count = 0\n    for row in shelf[1:]:\n        count += len(row)\n    return count\n";
    require_module_refusal(user_slice, "collect", "shelf[1:]")
}

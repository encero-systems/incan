//! Projections of the native-plan profile's `layout_records` shapes onto Crumb values.
//!
//! Each projection writes or reads one stored shape in its declared field and alternative order; record fields
//! are read from their identity-ordered slots. A shape here and its description in `wire_schema.rs` change
//! together.

use crate::records::prelude::*;

/// Project one `StoredLayoutEnumDeclaration` onto its schema-admitted Crumb record.
pub(crate) fn wire_enum_declaration(value: &EnumDeclaration) -> Result<Wire, String> {
    Ok(Wire::Record(vec![
        required(b"\x82\xa3S\x7f\xf0\xdb", Wire::Text((&value.name).clone())),
        required(b"\xef\xa1\xf3u\xd7a", Wire::Boolean(*&value.public)),
        required(b"\xda\xd5i\xe1\x01\xad", list(&value.variants, wire_enum_variant)?),
        required(
            b"\xa8\x8f\xe6\xeca\x15",
            list(&value.derives, |item0| Ok(Wire::Text((item0).clone())))?,
        ),
        required(b"7\xa0\x02Z\xa1\xf2", wire_source_span(&value.span)?),
        required(b"a5&\xd6\xca\x1f", Wire::integer(*&value.view_lifetime_count)),
        required(b"X\x03X~g\x91", wire_carrier(&value.carrier)?),
        required(b"\x18&\xb5\x1c\x02V", optional(&value.semantic_key, wire_checked_type)?),
        required(b"p6\x9c\xac\xa4\xd8", Wire::Text((&value.source_type).clone())),
        required(b"\xef\x06\xc8!DT", Wire::Boolean(*&value.member_debug)),
        required(b"\x8a\xac\xe8\x82\x8f\xe6", Wire::Text((&value.library_item).clone())),
        required(b".\xeeG\xf93\xc4", list(&value.library_type_arguments, wire_plan_type)?),
        required(b"\x1d\x91\xf5\x83}\xa5", Wire::Text((&value.raw_kind).clone())),
        required(
            b"\x0fF&\xf8\xea\xb1",
            list(&value.raw_values, |item0| Ok(Wire::Text((item0).clone())))?,
        ),
        required(
            b";\xbbc\xd8xg",
            list(&value.comparison_methods, |item0| Ok(Wire::Text((item0).clone())))?,
        ),
        required(
            b"\xb5\x1b\x90e\x9ax",
            list(&value.closure_items, |item0| Ok(Wire::Text((item0).clone())))?,
        ),
    ]))
}

/// Read one `StoredLayoutEnumDeclaration` record in its identity-ordered field slots.
pub(crate) fn read_enum_declaration(value: &Wire, positions: &WirePositions) -> Result<EnumDeclaration, String> {
    let fields = wire_fields(value, 16)?;
    Ok(EnumDeclaration {
        name: text(&fields[9].value)?,
        public: boolean_value(&fields[15].value)?,
        variants: read_list(&fields[13].value, |item0| read_enum_variant(item0, positions))?,
        derives: read_list(&fields[11].value, |item0| Ok(text(item0)?))?,
        span: read_source_span(&fields[4].value, positions)?,
        view_lifetime_count: number(&fields[7].value)?,
        carrier: read_carrier(&fields[6].value, positions)?,
        semantic_key: read_optional(&fields[1].value, |children| {
            Ok(read_checked_type(&children[0], positions)?)
        })?,
        source_type: text(&fields[8].value)?,
        member_debug: boolean_value(&fields[14].value)?,
        library_item: text(&fields[10].value)?,
        library_type_arguments: read_list(&fields[3].value, |item0| read_plan_type(item0, positions))?,
        raw_kind: text(&fields[2].value)?,
        raw_values: read_list(&fields[0].value, |item0| Ok(text(item0)?))?,
        comparison_methods: read_list(&fields[5].value, |item0| Ok(text(item0)?))?,
        closure_items: read_list(&fields[12].value, |item0| Ok(text(item0)?))?,
    })
}

/// Project one `StoredLayoutEnumVariant` onto its schema-admitted Crumb record.
pub(crate) fn wire_enum_variant(value: &EnumVariant) -> Result<Wire, String> {
    Ok(Wire::Record(vec![
        required(b"\x82\xa3S\x7f\xf0\xdb", Wire::Text((&value.name).clone())),
        required(b"\xbf\xe5\xd6\x97\x16-", list(&value.fields, wire_plan_type)?),
    ]))
}

/// Read one `StoredLayoutEnumVariant` record in its identity-ordered field slots.
pub(crate) fn read_enum_variant(value: &Wire, positions: &WirePositions) -> Result<EnumVariant, String> {
    let fields = wire_fields(value, 2)?;
    Ok(EnumVariant {
        name: text(&fields[0].value)?,
        fields: read_list(&fields[1].value, |item0| read_plan_type(item0, positions))?,
    })
}

/// Project one `StoredLayoutFallibleIteratorMethods` onto its schema-admitted Crumb record.
pub(crate) fn wire_fallible_iterator_methods(value: &FallibleIteratorMethods) -> Result<Wire, String> {
    Ok(Wire::Record(vec![
        required(b"J3\xea\xcd_\xa6", wire_plan_type(&value.item)?),
        required(b"\xca\x00\xfc\xcf\xb4\x08", wire_plan_type(&value.error)?),
        required(b"\xc6\xc1\xc9\xa9\xc8T", Wire::Text((&value.next).clone())),
    ]))
}

/// Read one `StoredLayoutFallibleIteratorMethods` record in its identity-ordered field slots.
pub(crate) fn read_fallible_iterator_methods(
    value: &Wire,
    positions: &WirePositions,
) -> Result<FallibleIteratorMethods, String> {
    let fields = wire_fields(value, 3)?;
    Ok(FallibleIteratorMethods {
        item: read_plan_type(&fields[0].value, positions)?,
        error: read_plan_type(&fields[2].value, positions)?,
        next: text(&fields[1].value)?,
    })
}

/// Project one `StoredLayoutFieldLifecycle` onto its schema-admitted Crumb record.
pub(crate) fn wire_field_lifecycle(value: &FieldLifecycle) -> Result<Wire, String> {
    Ok(Wire::Record(vec![
        required(
            b"kZ=\xed\x01j",
            optional(&value.since, |item0| Ok(Wire::Text((item0).clone())))?,
        ),
        required(
            b"\xf3C>\xc5V\xe6",
            optional(&value.retire_in, |item0| Ok(Wire::Text((item0).clone())))?,
        ),
        required(
            b"\xabS\x0a\x13\xe4Y",
            optional(&value.message, |item0| Ok(Wire::Text((item0).clone())))?,
        ),
        required(
            b"\xa7\xe2\xd2n\x8d\x15",
            optional(&value.by, |item0| Ok(Wire::Text((item0).clone())))?,
        ),
        required(
            b"JP(F\xd0p",
            optional(&value.issue, |item0| Ok(Wire::Text((item0).clone())))?,
        ),
    ]))
}

/// Read one `StoredLayoutFieldLifecycle` record in its identity-ordered field slots.
pub(crate) fn read_field_lifecycle(value: &Wire, positions: &WirePositions) -> Result<FieldLifecycle, String> {
    let fields = wire_fields(value, 5)?;
    Ok(FieldLifecycle {
        since: read_optional(&fields[1].value, |children| Ok(text(&children[0])?))?,
        retire_in: read_optional(&fields[4].value, |children| Ok(text(&children[0])?))?,
        message: read_optional(&fields[3].value, |children| Ok(text(&children[0])?))?,
        by: read_optional(&fields[2].value, |children| Ok(text(&children[0])?))?,
        issue: read_optional(&fields[0].value, |children| Ok(text(&children[0])?))?,
    })
}

/// Project one `StoredLayoutFieldMetadata` onto its schema-admitted Crumb record.
pub(crate) fn wire_field_metadata(value: &FieldMetadata) -> Result<Wire, String> {
    Ok(Wire::Record(vec![
        required(b"\x82\xa3S\x7f\xf0\xdb", Wire::Text((&value.name).clone())),
        required(
            b"E\xb0f\xb7\x9aM",
            list(&value.record_identity, |item0| Ok(Wire::integer(*item0)))?,
        ),
        required(b"\xbd\xeb\x90\xf5Pa", Wire::Boolean(*&value.record_canonical)),
        required(
            b"\xcdE\x7f.X\x1d",
            list(&value.renamed_from, |item0| Ok(Wire::Text((item0).clone())))?,
        ),
        required(
            b"\xcfZ4\xb47\x09",
            optional(&value.renamed_from_lifecycle, wire_field_lifecycle)?,
        ),
        required(b"ZE$YP;", Wire::integer(*&value.position)),
        required(b"\xda\xd0\x8fy8\xe2", Wire::Boolean(*&value.default_is_empty)),
        required(
            b"\x92\x7f\x03~3p",
            optional(&value.stored_type, |item0| Ok(Wire::Text((item0).clone())))?,
        ),
        required(
            b"\x1a\x0aj6\xca\x0a",
            optional(&value.alias, |item0| Ok(Wire::Text((item0).clone())))?,
        ),
        required(
            b"\xc9\x04oz7\xad",
            optional(&value.description, |item0| Ok(Wire::Text((item0).clone())))?,
        ),
        required(b"%\x01\xa7\xdd\xe4V", Wire::Text((&value.type_name).clone())),
        required(b"h*w|#(", Wire::Boolean(*&value.has_default)),
    ]))
}

/// Read one `StoredLayoutFieldMetadata` record in its identity-ordered field slots.
pub(crate) fn read_field_metadata(value: &Wire, positions: &WirePositions) -> Result<FieldMetadata, String> {
    let fields = wire_fields(value, 12)?;
    Ok(FieldMetadata {
        name: text(&fields[5].value)?,
        record_identity: read_list(&fields[2].value, |item0| Ok(number(item0)?))?,
        record_canonical: boolean_value(&fields[7].value)?,
        renamed_from: read_list(&fields[9].value, |item0| Ok(text(item0)?))?,
        renamed_from_lifecycle: read_optional(&fields[10].value, |children| {
            Ok(read_field_lifecycle(&children[0], positions)?)
        })?,
        position: number(&fields[3].value)?,
        default_is_empty: boolean_value(&fields[11].value)?,
        stored_type: read_optional(&fields[6].value, |children| Ok(text(&children[0])?))?,
        alias: read_optional(&fields[0].value, |children| Ok(text(&children[0])?))?,
        description: read_optional(&fields[8].value, |children| Ok(text(&children[0])?))?,
        type_name: text(&fields[1].value)?,
        has_default: boolean_value(&fields[4].value)?,
    })
}

/// Project one `StoredLayoutIteratorMethods` onto its schema-admitted Crumb record.
pub(crate) fn wire_iterator_methods(value: &IteratorMethods) -> Result<Wire, String> {
    Ok(Wire::Record(vec![
        required(b"J3\xea\xcd_\xa6", wire_plan_type(&value.item)?),
        required(b"\xc6\xc1\xc9\xa9\xc8T", Wire::Text((&value.next).clone())),
    ]))
}

/// Read one `StoredLayoutIteratorMethods` record in its identity-ordered field slots.
pub(crate) fn read_iterator_methods(value: &Wire, positions: &WirePositions) -> Result<IteratorMethods, String> {
    let fields = wire_fields(value, 2)?;
    Ok(IteratorMethods {
        item: read_plan_type(&fields[0].value, positions)?,
        next: text(&fields[1].value)?,
    })
}

/// Project one `StoredLayoutModelDeclaration` onto its schema-admitted Crumb record.
pub(crate) fn wire_model_declaration(value: &ModelDeclaration) -> Result<Wire, String> {
    Ok(Wire::Record(vec![
        required(b"\x82\xa3S\x7f\xf0\xdb", Wire::Text((&value.name).clone())),
        required(b"\xef\xa1\xf3u\xd7a", Wire::Boolean(*&value.public)),
        required(b"\xbf\xe5\xd6\x97\x16-", list(&value.fields, wire_parameter)?),
        required(
            b"\x0b\xc43\xf3\xaa\x0b",
            list(&value.field_public, |item0| Ok(Wire::Boolean(*item0)))?,
        ),
        required(
            b"\xa8\x8f\xe6\xeca\x15",
            list(&value.derives, |item0| Ok(Wire::Text((item0).clone())))?,
        ),
        required(b"7\xa0\x02Z\xa1\xf2", wire_source_span(&value.span)?),
        required(b"a5&\xd6\xca\x1f", Wire::integer(*&value.view_lifetime_count)),
        required(b"*\x12\xf5\x01h\xc9", Wire::Text((&value.source_name).clone())),
        required(b"\x8a\xac\xe8\x82\x8f\xe6", Wire::Text((&value.library_item).clone())),
        required(b".\xeeG\xf93\xc4", list(&value.library_type_arguments, wire_plan_type)?),
        required(
            b"=\x15#\x88O\xbb",
            list(&value.field_serde_names, |item0| Ok(Wire::Text((item0).clone())))?,
        ),
        required(
            b"\x16\x1d\xbe\xd0\x94`",
            Wire::Text((&value.checked_deserialize).clone()),
        ),
        required(
            b"\x1bO\xa3\x96\xb0\xa9",
            optional(&value.reflection_metadata, |item0| {
                Ok(list(item0, wire_field_metadata)?)
            })?,
        ),
        required(b"xX]\xef\x10\xd5", Wire::Boolean(*&value.field_metadata_carrier)),
        required(
            b";\xbbc\xd8xg",
            list(&value.comparison_methods, |item0| Ok(Wire::Text((item0).clone())))?,
        ),
        required(
            b"\xa9\xe5\xb8^:\xf8",
            optional(&value.fallible_iterator, wire_fallible_iterator_methods)?,
        ),
        required(
            b"\xef\x9b\xae\xdf\xf2\xf4",
            optional(&value.iterator, wire_iterator_methods)?,
        ),
        required(b"\xde*\xfbgVR", Wire::Text((&value.callable_method).clone())),
        required(b"Zv\x81SH\xae", list(&value.callable_signature, wire_plan_type)?),
        required(
            b"\xe1\xc2\xa1\x8c\xea\xaf",
            Wire::Text((&value.string_conversion_root).clone()),
        ),
        required(
            b"\x9d\x9e\x1a\x05i\x17",
            Wire::Text((&value.string_conversion_trait).clone()),
        ),
        required(
            b"\xd2y,\xbf\x1f&",
            list(&value.native_trait_adoptions, wire_native_trait_adoption)?,
        ),
        required(
            b"\x99\xc6EI\x8d\x81",
            list(&value.unavailable_native_fields, |item0| Ok(Wire::integer(*item0)))?,
        ),
    ]))
}

/// Read one `StoredLayoutModelDeclaration` record in its identity-ordered field slots.
pub(crate) fn read_model_declaration(value: &Wire, positions: &WirePositions) -> Result<ModelDeclaration, String> {
    let fields = wire_fields(value, 23)?;
    Ok(ModelDeclaration {
        name: text(&fields[11].value)?,
        public: boolean_value(&fields[22].value)?,
        fields: read_list(&fields[17].value, |item0| read_parameter(item0, positions))?,
        field_public: read_list(&fields[0].value, |item0| Ok(boolean_value(item0)?))?,
        derives: read_list(&fields[15].value, |item0| Ok(text(item0)?))?,
        span: read_source_span(&fields[5].value, positions)?,
        view_lifetime_count: number(&fields[9].value)?,
        source_name: text(&fields[3].value)?,
        library_item: text(&fields[12].value)?,
        library_type_arguments: read_list(&fields[4].value, |item0| read_plan_type(item0, positions))?,
        field_serde_names: read_list(&fields[7].value, |item0| Ok(text(item0)?))?,
        checked_deserialize: text(&fields[1].value)?,
        reflection_metadata: read_optional(&fields[2].value, |children| {
            Ok(read_list(&children[0], |item1| read_field_metadata(item1, positions))?)
        })?,
        field_metadata_carrier: boolean_value(&fields[10].value)?,
        comparison_methods: read_list(&fields[6].value, |item0| Ok(text(item0)?))?,
        fallible_iterator: read_optional(&fields[16].value, |children| {
            Ok(read_fallible_iterator_methods(&children[0], positions)?)
        })?,
        iterator: read_optional(&fields[21].value, |children| {
            Ok(read_iterator_methods(&children[0], positions)?)
        })?,
        callable_method: text(&fields[19].value)?,
        callable_signature: read_list(&fields[8].value, |item0| read_plan_type(item0, positions))?,
        string_conversion_root: text(&fields[20].value)?,
        string_conversion_trait: text(&fields[14].value)?,
        native_trait_adoptions: read_list(&fields[18].value, |item0| read_native_trait_adoption(item0, positions))?,
        unavailable_native_fields: read_list(&fields[13].value, |item0| Ok(number(item0)?))?,
    })
}

/// Project one `StoredLayoutNativeTraitAdoption` onto its schema-admitted Crumb record.
pub(crate) fn wire_native_trait_adoption(value: &NativeTraitAdoption) -> Result<Wire, String> {
    Ok(Wire::Record(vec![
        required(b"\xa0\xaf\x9f\x86[\xf6", Wire::Text((&value.path).clone())),
        required(b"?8g1:\x9d", list(&value.methods, wire_native_trait_method)?),
    ]))
}

/// Read one `StoredLayoutNativeTraitAdoption` record in its identity-ordered field slots.
pub(crate) fn read_native_trait_adoption(
    value: &Wire,
    positions: &WirePositions,
) -> Result<NativeTraitAdoption, String> {
    let fields = wire_fields(value, 2)?;
    Ok(NativeTraitAdoption {
        path: text(&fields[1].value)?,
        methods: read_list(&fields[0].value, |item0| read_native_trait_method(item0, positions))?,
    })
}

/// Project one `StoredLayoutNativeTraitMethod` onto its schema-admitted Crumb record.
pub(crate) fn wire_native_trait_method(value: &NativeTraitMethod) -> Result<Wire, String> {
    Ok(Wire::Record(vec![
        required(b"\x82\xa3S\x7f\xf0\xdb", Wire::Text((&value.name).clone())),
        required(b"x\xf9\xac\x01\x8eU", Wire::Text((&value.function).clone())),
    ]))
}

/// Read one `StoredLayoutNativeTraitMethod` record in its identity-ordered field slots.
pub(crate) fn read_native_trait_method(value: &Wire, positions: &WirePositions) -> Result<NativeTraitMethod, String> {
    let fields = wire_fields(value, 2)?;
    Ok(NativeTraitMethod {
        name: text(&fields[1].value)?,
        function: text(&fields[0].value)?,
    })
}

/// Project one `StoredLayoutParameter` onto its schema-admitted Crumb record.
pub(crate) fn wire_parameter(value: &Parameter) -> Result<Wire, String> {
    Ok(Wire::Record(vec![
        required(b"\x82\xa3S\x7f\xf0\xdb", Wire::Text((&value.name).clone())),
        required(b"O\x9d\xb7\xf8\xe5\x06", wire_plan_type(&value.ty)?),
        required(b"7\xa0\x02Z\xa1\xf2", wire_source_span(&value.span)?),
    ]))
}

/// Read one `StoredLayoutParameter` record in its identity-ordered field slots.
pub(crate) fn read_parameter(value: &Wire, positions: &WirePositions) -> Result<Parameter, String> {
    let fields = wire_fields(value, 3)?;
    Ok(Parameter {
        name: text(&fields[2].value)?,
        ty: read_plan_type(&fields[1].value, positions)?,
        span: read_source_span(&fields[0].value, positions)?,
    })
}

/// Project one `StoredLayoutZipCarrier` onto its schema-admitted Crumb record.
pub(crate) fn wire_zip_carrier(value: &ZipCarrier) -> Result<Wire, String> {
    Ok(Wire::Record(vec![
        required(b"6\x0f\x84\x03YB", wire_tuple_element(&value.left)?),
        required(b"'\x04/Nn\xca", wire_tuple_element(&value.right)?),
        required(b"4\xa0@\x05\xbc\xaf", wire_plan_type(&value.target)?),
    ]))
}

/// Read one `StoredLayoutZipCarrier` record in its identity-ordered field slots.
pub(crate) fn read_zip_carrier(value: &Wire, positions: &WirePositions) -> Result<ZipCarrier, String> {
    let fields = wire_fields(value, 3)?;
    Ok(ZipCarrier {
        left: read_tuple_element(&fields[2].value, positions)?,
        right: read_tuple_element(&fields[0].value, positions)?,
        target: read_plan_type(&fields[1].value, positions)?,
    })
}

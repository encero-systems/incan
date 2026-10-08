# Direct-route fixture census

1101 fixtures. Pending fixtures retain their observed result in JSON; refusal ranks include pending observations. Check-only details report verification failures without attributing them to the direct route.

| Class | Count |
| --- | ---: |
| pass | 412 |
| refused | 545 |
| wrong | 0 |
| driver-error | 0 |
| check-only | 131 |
| multi-module | 0 |
| pending | 13 |

## Areas

| Area | pass | refused | wrong | driver-error | check-only | multi-module | pending |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| cli_dependencies | 0 | 20 | 0 | 0 | 5 | 0 | 0 |
| cli_logic_comparisons_and_patterns | 1 | 11 | 0 | 0 | 0 | 0 | 0 |
| cli_modules_and_declarations | 8 | 28 | 0 | 0 | 0 | 0 | 0 |
| cli_refusals | 0 | 0 | 0 | 0 | 48 | 0 | 0 |
| cli_refusals_mutation | 0 | 0 | 0 | 0 | 22 | 0 | 0 |
| cli_refusals_values_and_calls | 0 | 0 | 0 | 0 | 17 | 0 | 0 |
| cli_values_and_calls | 7 | 45 | 0 | 0 | 0 | 0 | 0 |
| codegen_dependencies | 0 | 2 | 0 | 0 | 0 | 0 | 0 |
| codegen_functions_and_aliases | 3 | 11 | 0 | 0 | 1 | 0 | 0 |
| codegen_models_and_collections | 7 | 20 | 0 | 0 | 0 | 0 | 0 |
| codegen_modules_and_imports | 7 | 9 | 0 | 0 | 1 | 0 | 0 |
| driver | 3 | 0 | 0 | 0 | 0 | 0 | 0 |
| emit_calls_and_builtins | 7 | 35 | 0 | 0 | 0 | 0 | 0 |
| emit_declarations_and_modules | 3 | 12 | 0 | 0 | 3 | 0 | 0 |
| emit_dependency_unions_and_surfaces | 0 | 3 | 0 | 0 | 0 | 0 | 0 |
| execution_calls_types_and_modules | 77 | 36 | 0 | 0 | 4 | 0 | 1 |
| execution_collections | 33 | 32 | 0 | 0 | 10 | 0 | 6 |
| execution_numerics_and_conversions | 60 | 9 | 0 | 0 | 1 | 0 | 1 |
| execution_scalars_and_bodies | 60 | 12 | 0 | 0 | 4 | 0 | 4 |
| execution_strings_and_output | 70 | 2 | 0 | 0 | 0 | 0 | 1 |
| harness | 5 | 1 | 0 | 0 | 2 | 0 | 0 |
| lowering_declarations | 6 | 25 | 0 | 0 | 2 | 0 | 0 |
| lowering_dependencies | 2 | 4 | 0 | 0 | 0 | 0 | 0 |
| lowering_expressions | 2 | 19 | 0 | 0 | 2 | 0 | 0 |
| ownership_numerics_and_bounds | 1 | 8 | 0 | 0 | 0 | 0 | 0 |
| ownership_parameters_and_receivers | 0 | 14 | 0 | 0 | 1 | 0 | 0 |
| ownership_string_boundaries | 2 | 4 | 0 | 0 | 1 | 0 | 0 |
| ownership_values_and_fields | 3 | 10 | 0 | 0 | 0 | 0 | 0 |
| smoke | 5 | 0 | 0 | 0 | 0 | 0 | 0 |
| snapshots_collections_and_strings | 7 | 34 | 0 | 0 | 1 | 0 | 0 |
| snapshots_enums_and_matching | 3 | 16 | 0 | 0 | 0 | 0 | 0 |
| snapshots_functions_and_projections | 9 | 19 | 0 | 0 | 0 | 0 | 0 |
| snapshots_models_and_classes | 7 | 14 | 0 | 0 | 0 | 0 | 0 |
| snapshots_newtypes_and_serde | 4 | 16 | 0 | 0 | 0 | 0 | 0 |
| snapshots_stdlib | 3 | 34 | 0 | 0 | 1 | 0 | 0 |
| snapshots_traits_and_generics | 1 | 29 | 0 | 0 | 5 | 0 | 0 |
| snapshots_values_and_control_flow | 6 | 11 | 0 | 0 | 0 | 0 | 0 |

## Refusal constructs

| Construct | Count |
| --- | ---: |
| unsupported Body IR type TypeVar | 36 |
| unsupported Body IR type Function | 29 |
| unsupported source generic Trait, supertraits, decorators, aliases or properties on the native route | 22 |
| unsupported Body IR type Iterator[ | 20 |
| unsupported Body IR type Unknown | 20 |
| unsupported source Static initializer or carrier on the native route | 14 |
| unsupported Body IR Match outside source enums | 11 |
| unsupported Body IR Method without canonical identity | 11 |
| unsupported Body IR owned-value branch merge | 11 |
| unsupported source nonplain Newtype on the native route | 11 |
| unsupported Body IR builtin Enumerate | 10 |
| unsupported Body IR nominal type IoError | 7 |
| unsupported Body IR nonmodel place projection | 7 |
| unsupported source Class generic trait adoptions, inheritance, type parameters, decorators, aliases, properties or defaults on Box on the native route | 7 |
| unsupported Body IR defaulted parameter type | 6 |
| unsupported Body IR nonlocal or trait-object Method | 6 |
| unsupported Body IR primitive type FrozenStr | 6 |
| unsupported Body IR type Decimal | 6 |
| unsupported source Partial on the native route | 6 |
| unsupported Body IR builtin Range | 5 |
| unsupported Body IR imported or unresolved canonical call target | 5 |
| unsupported Body IR nominal type TaskJoinError | 5 |
| unsupported Body IR nonunion enum isinstance | 5 |
| unsupported source checked Newtype construction on the native route | 5 |
| unsupported Body IR display type | 4 |
| unsupported Body IR list scalar operator | 4 |
| unsupported Body IR mixed numeric arithmetic without retained coercion | 4 |
| unsupported Body IR model JSON serialization without serde::Serialize | 4 |
| unsupported Body IR mutable model parameter | 4 |
| unsupported Body IR nested model projection | 4 |
| unsupported Body IR nonmodel FormatStyle.Debug | 4 |
| unsupported Body IR tuple element type List[int] | 4 |
| unsupported Body IR tuple element type TypeVar | 4 |
| unsupported source Enum Level on the native route | 4 |
| unsupported source Model decorator @derive | 4 |
| unsupported source Model type parameters on Box on the native route | 4 |
| unsupported Body IR ExternDelegation | 3 |
| unsupported Body IR Unsupported: `unsafe:` acknowledgment region: refused by design, because Body IR v0 cannot carry the acknowledgment a consumer would need to admit it deliberately | 3 |
| unsupported Body IR collection leaf List[int] | 3 |
| unsupported Body IR list Method on non-list | 3 |
| unsupported Body IR list spread | 3 |
| unsupported Body IR mutable String parameter | 3 |
| unsupported Body IR nominal type CompressionError | 3 |
| unsupported Body IR nominal type EnvironError | 3 |
| unsupported Body IR non-float true division operands | 3 |
| unsupported Body IR type Range[int] | 3 |
| unsupported source Model method aliases on User on the native route | 3 |
| unsupported source Model type parameters on Holder on the native route | 3 |
| unsupported Body IR Generator | 2 |
| unsupported Body IR builtin or unproven NamedCallableTarget | 2 |
| unsupported Body IR collection leaf Tag | 2 |
| unsupported Body IR enum pattern | 2 |
| unsupported Body IR external async future | 2 |
| unsupported Body IR list slice or field projection | 2 |
| unsupported Body IR mixed Pow operands | 2 |
| unsupported Body IR mutable Method argument | 2 |
| unsupported Body IR nominal type FieldInfo | 2 |
| unsupported Body IR nominal type JsonValue | 2 |
| unsupported Body IR nominal type OrdinalMapError | 2 |
| unsupported Body IR nominal type ValidationError | 2 |
| unsupported Body IR non-condition Assert | 2 |
| unsupported Body IR non-list borrow | 2 |
| unsupported Body IR non-unit native main | 2 |
| unsupported Body IR optional dictionary get | 2 |
| unsupported Body IR package executable representation: package `fallible_streams` version 0.1.0 cannot satisfy the executable representation requirement: this package publishes no executable representation for `NumberStream` | 2 |
| unsupported Body IR package executable representation: package `modulelib` version 0.1.0 cannot satisfy the executable representation requirement: this package publishes no executable representation for `default_index` | 2 |
| unsupported Body IR static ownership | 2 |
| unsupported Body IR type FrozenList[str] | 2 |
| unsupported Body IR type Option[List[int]] | 2 |
| unsupported Body IR type RustInteropPath | 2 |
| unsupported Body IR type SendError[TypeVar] | 2 |
| unsupported Body IR type Union[int, str] | 2 |
| unsupported Body IR union member List[int]: unsupported Body IR union member | 2 |
| unsupported source Class generic trait adoptions, inheritance, type parameters, decorators, aliases, properties or defaults on Counter on the native route | 2 |
| unsupported source Class generic trait adoptions, inheritance, type parameters, decorators, aliases, properties or defaults on Cursor on the native route | 2 |
| unsupported source Class generic trait adoptions, inheritance, type parameters, decorators, aliases, properties or defaults on Vault on the native route | 2 |
| unsupported source Model derived Default over field defaults on Settings on the native route | 2 |
| unsupported source Model generic or decorated methods on Client on the native route | 2 |
| unsupported source Model generic trait adoption on Count on the native route | 2 |
| unsupported source Model generic trait adoption on NumberStream on the native route | 2 |
| unsupported source Model generic trait adoption on Observer on the native route | 2 |
| unsupported source Model serde field aliases on Account on the native route | 2 |
| unsupported source Model type parameters on Boxed on the native route | 2 |
| unsupported source Model type parameters on Column on the native route | 2 |
| unsupported source Model type parameters on Pair on the native route | 2 |
| unsupported source Model type parameters on Stream on the native route | 2 |
| unsupported source import this entrypoint effect on the native route | 2 |
| unsupported Body IR BitAnd | 1 |
| unsupported Body IR BitOr | 1 |
| unsupported Body IR ambiguous project nominal carrier Extent | 1 |
| unsupported Body IR ambiguous project nominal carrier Product | 1 |
| unsupported Body IR ambiguous project nominal carrier Size | 1 |
| unsupported Body IR collection leaf ? | 1 |
| unsupported Body IR collection leaf Counter | 1 |
| unsupported Body IR collection leaf Dict[str, int] | 1 |
| unsupported Body IR collection leaf Holder | 1 |
| unsupported Body IR collection leaf Option[Union[bool, int]] | 1 |
| unsupported Body IR collection leaf T | 1 |
| unsupported Body IR enum match branch ownership change | 1 |
| unsupported Body IR generator yield coercion | 1 |
| unsupported Body IR global list place | 1 |
| unsupported Body IR indexing non-list | 1 |
| unsupported Body IR len on non-list | 1 |
| unsupported Body IR list root projection | 1 |
| unsupported Body IR mutation of borrowed list iteration item | 1 |
| unsupported Body IR nested receiver projection | 1 |
| unsupported Body IR nominal type A | 1 |
| unsupported Body IR nominal type ConformanceRel | 1 |
| unsupported Body IR nominal type DateTimeError | 1 |
| unsupported Body IR nominal type Drawable | 1 |
| unsupported Body IR nominal type EncodingError | 1 |
| unsupported Body IR nominal type ExprKind | 1 |
| unsupported Body IR nominal type FixedOffset | 1 |
| unsupported Body IR nominal type HashError | 1 |
| unsupported Body IR nominal type Html | 1 |
| unsupported Body IR nominal type MemberAlias | 1 |
| unsupported Body IR nominal type MiddleTone | 1 |
| unsupported Body IR nominal type Node | 1 |
| unsupported Body IR nominal type NodeId | 1 |
| unsupported Body IR nominal type Path | 1 |
| unsupported Body IR nominal type PublicVault | 1 |
| unsupported Body IR nominal type PublicWidget | 1 |
| unsupported Body IR nominal type RegexError | 1 |
| unsupported Body IR nominal type RootKind | 1 |
| unsupported Body IR nominal type Sha256Hasher | 1 |
| unsupported Body IR nominal type TelemetryValue | 1 |
| unsupported Body IR nominal type Time | 1 |
| unsupported Body IR nominal type UuidError | 1 |
| unsupported Body IR nominal type pub::hyper::ExactResult | 1 |
| unsupported Body IR nominal type pub::querykit::IntLiteralExpr | 1 |
| unsupported Body IR overloaded function convert | 1 |
| unsupported Body IR overloaded function pick | 1 |
| unsupported Body IR ownership Borrow outside formatting | 1 |
| unsupported Body IR package executable representation: package `bounds` version 0.1.0 cannot satisfy the executable representation requirement: this package publishes no executable representation for `Holder` | 1 |
| unsupported Body IR package executable representation: package `catalogs` version 0.1.0 cannot satisfy the executable representation requirement: this package publishes no executable representation for `Parcel` | 1 |
| unsupported Body IR package executable representation: package `flows` version 0.1.0 cannot satisfy the executable representation requirement: this package publishes no executable representation for `Phase` | 1 |
| unsupported Body IR package executable representation: package `kinds` version 0.1.0 cannot satisfy the executable representation requirement: this package publishes no executable representation for `current_count` | 1 |
| unsupported Body IR package executable representation: package `modulelib` version 0.1.0 cannot satisfy the executable representation requirement: this package publishes no executable representation for `IndexBuilder` | 1 |
| unsupported Body IR package executable representation: package `modulelib` version 0.1.0 cannot satisfy the executable representation requirement: this package publishes no executable representation for `show_ace` | 1 |
| unsupported Body IR package executable representation: package `recall` version 0.1.0 cannot satisfy the executable representation requirement: this package publishes no executable representation for `EntryId` | 1 |
| unsupported Body IR package executable representation: package `shapes` version 0.1.0 cannot satisfy the executable representation requirement: this package publishes no executable representation for `build` | 1 |
| unsupported Body IR package executable representation: package `shapes` version 0.1.0 cannot satisfy the executable representation requirement: this package publishes no executable representation for `count` | 1 |
| unsupported Body IR package executable representation: package `unionlib` version 0.1.0 cannot satisfy the executable representation requirement: this package publishes no executable representation for `Choice` | 1 |
| unsupported Body IR package executable representation: package `widgets` version 0.1.0 cannot satisfy the executable representation requirement: this package publishes no executable representation for `Envelope` | 1 |
| unsupported Body IR project without selected main entrypoint | 1 |
| unsupported Body IR tuple element type List[bool] | 1 |
| unsupported Body IR tuple element type Named[Cell] | 1 |
| unsupported Body IR type Deque[str] | 1 |
| unsupported Body IR type FrozenList[Named[FieldInfo]] | 1 |
| unsupported Body IR type FrozenList[int] | 1 |
| unsupported Body IR type Json[Named[User]] | 1 |
| unsupported Body IR type Option[int] | 1 |
| unsupported Body IR type Path[int] | 1 |
| unsupported Body IR type Query[Named[Search]] | 1 |
| unsupported Body IR type SelfType | 1 |
| unsupported Body IR type SendError[str] | 1 |
| unsupported Body IR type Sender[int] | 1 |
| unsupported Body IR type Union[Named[ColumnRefExpr], Named[ScalarFunctionExpr]] | 1 |
| unsupported Body IR type Union[Named[IntExpr], str] | 1 |
| unsupported Body IR type Union[Named[LeftValue], Named[RightValue]] | 1 |
| unsupported Body IR unary operator Invert | 1 |
| unsupported Body IR unresolved ArgumentBinding | 1 |
| unsupported Body IR unretained global storage `LOWEST` | 1 |
| unsupported Body IR unretained global storage `NAN` | 1 |
| unsupported Body IR unretained global storage `PI` | 1 |
| unsupported source Class generic trait adoptions, inheritance, type parameters, decorators, aliases, properties or defaults on Account on the native route | 1 |
| unsupported source Class generic trait adoptions, inheritance, type parameters, decorators, aliases, properties or defaults on Boxed on the native route | 1 |
| unsupported source Class generic trait adoptions, inheritance, type parameters, decorators, aliases, properties or defaults on Example on the native route | 1 |
| unsupported source Class generic trait adoptions, inheritance, type parameters, decorators, aliases, properties or defaults on Factory on the native route | 1 |
| unsupported source Class generic trait adoptions, inheritance, type parameters, decorators, aliases, properties or defaults on FactoryBox on the native route | 1 |
| unsupported source Class generic trait adoptions, inheritance, type parameters, decorators, aliases, properties or defaults on GenericBox on the native route | 1 |
| unsupported source Class generic trait adoptions, inheritance, type parameters, decorators, aliases, properties or defaults on Index on the native route | 1 |
| unsupported source Class generic trait adoptions, inheritance, type parameters, decorators, aliases, properties or defaults on ProbeRegistry on the native route | 1 |
| unsupported source Class generic trait adoptions, inheritance, type parameters, decorators, aliases, properties or defaults on Record on the native route | 1 |
| unsupported source Enum Env on the native route | 1 |
| unsupported source Enum Holder on the native route | 1 |
| unsupported source Enum Shape on the native route | 1 |
| unsupported source Enum Signal on the native route | 1 |
| unsupported source Enum Suit on the native route | 1 |
| unsupported source Model derived Default over field defaults on Point on the native route | 1 |
| unsupported source Model generic or decorated methods on Account on the native route | 1 |
| unsupported source Model generic or decorated methods on AllowModel on the native route | 1 |
| unsupported source Model generic or decorated methods on Codec on the native route | 1 |
| unsupported source Model generic or decorated methods on Counter on the native route | 1 |
| unsupported source Model generic or decorated methods on FieldInfo on the native route | 1 |
| unsupported source Model generic or decorated methods on Page on the native route | 1 |
| unsupported source Model generic or decorated methods on Picker on the native route | 1 |
| unsupported source Model generic or decorated methods on Response on the native route | 1 |
| unsupported source Model generic or decorated methods on Session on the native route | 1 |
| unsupported source Model generic or decorated methods on TimeDelta on the native route | 1 |
| unsupported source Model generic trait adoption on Countdown on the native route | 1 |
| unsupported source Model generic trait adoption on Money on the native route | 1 |
| unsupported source Model generic trait adoption on Prefixer on the native route | 1 |
| unsupported source Model generic trait adoption on Readings on the native route | 1 |
| unsupported source Model generic trait adoption on Ticks on the native route | 1 |
| unsupported source Model generic trait adoption on UserId on the native route | 1 |
| unsupported source Model generic trait adoption on Window on the native route | 1 |
| unsupported source Model method aliases on Reading on the native route | 1 |
| unsupported source Model method aliases on Stats on the native route | 1 |
| unsupported source Model method partials on Cell on the native route | 1 |
| unsupported source Model method partials on Greeter on the native route | 1 |
| unsupported source Model properties on Counter on the native route | 1 |
| unsupported source Model properties on Pair on the native route | 1 |
| unsupported source Model type parameters on Chunks on the native route | 1 |
| unsupported source Model type parameters on Decoded on the native route | 1 |
| unsupported source Model type parameters on Json on the native route | 1 |
| unsupported source Model type parameters on Wrapper on the native route | 1 |

## Former directory-fixture exclusions

| Fixture | Class | Detail |
| --- | --- | --- |
| cli_dependencies/callable_across_package_boundary | refused | unsupported Body IR type Function |
| cli_dependencies/compiled_generic_bounds | refused | unsupported Body IR package executable representation: package `bounds` version 0.1.0 cannot satisfy the executable representation requirement: this package publishes no executable representation for `Holder` |
| cli_dependencies/dependency_enums_sharing_variant_names | refused | unsupported Body IR package executable representation: package `flows` version 0.1.0 cannot satisfy the executable representation requirement: this package publishes no executable representation for `Phase` |
| cli_dependencies/dependency_helper_defaults_and_unions | refused | unsupported Body IR nominal type pub::querykit::IntLiteralExpr |
| cli_dependencies/dependency_method_returning_a_union | refused | unsupported Body IR type Union[int, str] |
| cli_dependencies/dependency_model_built_with_empty_fields_of_unimported_types | refused | unsupported Body IR collection leaf Tag |
| cli_dependencies/dependency_reexported_type_identity | refused | unsupported Body IR package executable representation: package `recall` version 0.1.0 cannot satisfy the executable representation requirement: this package publishes no executable representation for `EntryId` |
| cli_dependencies/dependency_renamed_export_reexported_under_a_new_name | refused | unsupported Body IR type Unknown |
| cli_dependencies/dependency_same_named_declarations_in_two_modules | refused | unsupported Body IR ambiguous project nominal carrier Product |
| cli_dependencies/dependency_submodule_reexports | refused | unsupported Body IR nominal type MiddleTone |
| cli_dependencies/dependency_trait_adopted_and_bound_under_different_spellings | refused | unsupported Body IR type TypeVar |
| cli_dependencies/dependency_trait_through_a_module_binding | refused | unsupported Body IR type TypeVar |
| cli_dependencies/dependency_trait_under_another_spelling | refused | unsupported Body IR type TypeVar |
| cli_dependencies/dependency_union_surface_keeps_reexported_element_fields | refused | unsupported Body IR nominal type pub::hyper::ExactResult |
| cli_dependencies/fallible_iterator_chain_without_trait_import | refused | unsupported Body IR package executable representation: package `fallible_streams` version 0.1.0 cannot satisfy the executable representation requirement: this package publishes no executable representation for `NumberStream` |
| cli_dependencies/imported_partial_keeps_target_defaults | refused | unsupported Body IR package executable representation: package `modulelib` version 0.1.0 cannot satisfy the executable representation requirement: this package publishes no executable representation for `default_index` |
| cli_dependencies/mut_parameter_across_package_boundary | refused | unsupported Body IR mutable model parameter |
| cli_dependencies/private_default_items_from_dependency | refused | unsupported Body IR package executable representation: package `shapes` version 0.1.0 cannot satisfy the executable representation requirement: this package publishes no executable representation for `build` |
| cli_dependencies/public_construction_default_from_dependency | refused | unsupported Body IR package executable representation: package `shapes` version 0.1.0 cannot satisfy the executable representation requirement: this package publishes no executable representation for `count` |
| cli_dependencies/some_member_into_dependency_option_union | refused | unsupported Body IR package executable representation: package `unionlib` version 0.1.0 cannot satisfy the executable representation requirement: this package publishes no executable representation for `Choice` |
| cli_modules_and_declarations/default_static_reads_across_modules | refused | unsupported source Class generic trait adoptions, inheritance, type parameters, decorators, aliases, properties or defaults on Box on the native route |
| cli_modules_and_declarations/external_impl_bound_reaches_a_generic_caller | refused | unsupported source Model type parameters on Holder on the native route |
| cli_modules_and_declarations/facade_reexport_alias_target | refused | unsupported Body IR type Union[Named[ColumnRefExpr], Named[ScalarFunctionExpr]] |
| cli_modules_and_declarations/facade_reexports_an_alias_without_its_target | pass |  |
| cli_modules_and_declarations/gen_named_function_field_and_module | pass |  |
| cli_modules_and_declarations/imported_static_decorator_receiver | refused | unsupported source Class generic trait adoptions, inheritance, type parameters, decorators, aliases, properties or defaults on ProbeRegistry on the native route |
| cli_modules_and_declarations/module_derive_default_method_direct_call | refused | unsupported source generic Trait, supertraits, decorators, aliases or properties on the native route |
| cli_modules_and_declarations/module_member_alias_beside_a_direct_import | pass |  |
| cli_modules_and_declarations/partial_pattern_over_imported_private_field | refused | unsupported source Model generic or decorated methods on Account on the native route |
| cli_modules_and_declarations/private_default_items_across_modules | refused | unsupported source Model method partials on Greeter on the native route |
| cli_modules_and_declarations/relative_import_from_a_nested_module | pass |  |
| cli_modules_and_declarations/same_named_default_consts_across_modules | pass |  |
| cli_modules_and_declarations/trait_default_constructs_module_type_across_modules | refused | unsupported Body IR nonlocal or trait-object Method |
| cli_modules_and_declarations/trait_default_generic_element_read_across_modules | refused | unsupported source generic Trait, supertraits, decorators, aliases or properties on the native route |
| cli_modules_and_declarations/trait_default_names_its_own_module_types | refused | unsupported Body IR nonlocal or trait-object Method |
| cli_modules_and_declarations/trait_default_reaches_its_module_names_across_modules | refused | unsupported Body IR nonlocal or trait-object Method |
| cli_values_and_calls/local_partials_of_generic_and_imported_callables | refused | unsupported source Model type parameters on Box on the native route |
| codegen_dependencies/implied_generic_supertrait_adoption | refused | unsupported Body IR package executable representation: package `catalogs` version 0.1.0 cannot satisfy the executable representation requirement: this package publishes no executable representation for `Parcel` |
| codegen_dependencies/pub_widgets_imports | refused | unsupported Body IR nominal type PublicWidget |
| codegen_modules_and_imports/dependency_module_public_items | pass |  |
| codegen_modules_and_imports/facade_reexported_serde_trait | refused | unsupported Body IR model JSON serialization without serde::Serialize |
| codegen_modules_and_imports/field_alias_across_modules | refused | unsupported Body IR nominal type A |
| codegen_modules_and_imports/flat_module_import | pass |  |
| codegen_modules_and_imports/module_derive_through_import_binding | refused | unsupported source generic Trait, supertraits, decorators, aliases or properties on the native route |
| codegen_modules_and_imports/module_item_import_spellings | pass |  |
| codegen_modules_and_imports/nested_decorated_generic_reflection | refused | unsupported Body IR type FrozenList[Named[FieldInfo]] |
| codegen_modules_and_imports/nested_module_import_spellings | pass |  |
| codegen_modules_and_imports/qualified_type_annotations | refused | unsupported Body IR nominal type FieldInfo |
| codegen_modules_and_imports/reexported_partial_and_static | refused | unsupported source Partial on the native route |
| codegen_modules_and_imports/subtrait_bound_calls_unimported_supertrait_method | refused | unsupported source generic Trait, supertraits, decorators, aliases or properties on the native route |
| codegen_modules_and_imports/user_types_named_like_stdlib_types | pass |  |
| driver/module_named_like_host_keyword | pass |  |
| driver/module_named_like_host_root_file | pass |  |
| emit_declarations_and_modules/constructor_arguments_name_field_types_the_module_does_not_import | refused | unsupported source nonplain Newtype on the native route |
| emit_declarations_and_modules/default_argument_calls_sibling_module | pass |  |
| emit_declarations_and_modules/dependency_model_generic_method_string_args | refused | unsupported source Model generic or decorated methods on Session on the native route |
| emit_declarations_and_modules/imported_model_constructor_default_fill | refused | unsupported Body IR type Unknown |
| emit_declarations_and_modules/module_qualified_field_chains_and_calls | refused | unsupported Body IR static ownership |
| emit_declarations_and_modules/nested_module_union_enum_and_none_patterns | pass |  |
| emit_declarations_and_modules/private_default_fields_across_modules | refused | unsupported source Class generic trait adoptions, inheritance, type parameters, decorators, aliases, properties or defaults on Vault on the native route |
| emit_dependency_unions_and_surfaces/dependency_constructor_surfaces | refused | unsupported Body IR package executable representation: package `modulelib` version 0.1.0 cannot satisfy the executable representation requirement: this package publishes no executable representation for `IndexBuilder` |
| emit_dependency_unions_and_surfaces/dependency_declaration_kinds | refused | unsupported Body IR package executable representation: package `kinds` version 0.1.0 cannot satisfy the executable representation requirement: this package publishes no executable representation for `current_count` |
| emit_dependency_unions_and_surfaces/dependency_unions_across_the_package_boundary | refused | unsupported Body IR type Union[Named[IntExpr], str] |
| execution_calls_types_and_modules/a_class_in_a_sibling_module_beside_the_called_function | pass |  |
| execution_calls_types_and_modules/a_feature_gated_main_left_unselected | refused | unsupported Body IR project without selected main entrypoint |
| execution_calls_types_and_modules/a_feature_gated_main_selected_by_default | pass |  |
| execution_calls_types_and_modules/a_rust_import_in_an_imported_module | pass |  |
| execution_calls_types_and_modules/call_into_a_module_lowered_on_its_own | pass |  |
| execution_calls_types_and_modules/call_into_a_sibling_module | pass |  |
| execution_calls_types_and_modules/facade_chain_shares_one_declaration | pass |  |
| execution_calls_types_and_modules/module_qualified_call | pass |  |
| execution_calls_types_and_modules/module_qualified_call_through_an_alias | pass |  |
| execution_calls_types_and_modules/parity_names_reached_locally_imported_aliased_and_reexported | refused | unsupported Body IR nominal type MemberAlias |
| execution_calls_types_and_modules/parity_unselected_feature_function_is_absent | pass |  |
| execution_calls_types_and_modules/typed_list_in_a_callee_module | pass |  |
| execution_calls_types_and_modules/typed_numeric_call_into_its_declaring_module | pass |  |
| harness/modules | pass |  |
| harness/project | pass |  |
| lowering_declarations/adopted_serde_traits_by_owner | refused | unsupported Body IR nonlocal or trait-object Method |
| lowering_declarations/comparison_dunders_of_an_adopter_from_another_module | refused | unsupported source Enum Level on the native route |
| lowering_declarations/default_type_constructors_keep_module_identity | refused | unsupported Body IR ambiguous project nominal carrier Size |
| lowering_declarations/imported_union_member_identity | pass |  |
| lowering_declarations/imported_user_json_keeps_field | refused | unsupported source Model type parameters on Json on the native route |
| lowering_declarations/local_trait_shadows_imported_trait_issue1592 | refused | unsupported Body IR enum match branch ownership change |
| lowering_declarations/trait_default_local_shadows_module_type | refused | unsupported Body IR nonlocal or trait-object Method |
| lowering_declarations/trait_default_nested_module_identity | refused | unsupported Body IR ambiguous project nominal carrier Extent |
| lowering_dependencies/dependency_partial_and_function_value | refused | unsupported Body IR package executable representation: package `modulelib` version 0.1.0 cannot satisfy the executable representation requirement: this package publishes no executable representation for `show_ace` |
| lowering_dependencies/fallible_iterator_through_public_library | refused | unsupported Body IR package executable representation: package `fallible_streams` version 0.1.0 cannot satisfy the executable representation requirement: this package publishes no executable representation for `NumberStream` |
| lowering_dependencies/identity_graph_alias_signature | pass |  |
| lowering_dependencies/package_inherent_methods_issue1174 | pass |  |
| lowering_dependencies/parent_namespace_and_qualified_partial_issue948 | refused | unsupported Body IR package executable representation: package `modulelib` version 0.1.0 cannot satisfy the executable representation requirement: this package publishes no executable representation for `default_index` |
| lowering_dependencies/provider_nominal_types_and_unions_issue892 | refused | unsupported Body IR package executable representation: package `widgets` version 0.1.0 cannot satisfy the executable representation requirement: this package publishes no executable representation for `Envelope` |
| lowering_expressions/callable_trait_local_versus_std | refused | unsupported source generic Trait, supertraits, decorators, aliases or properties on the native route |
| ownership_numerics_and_bounds/local_function_named_like_a_generic_in_another_module | refused | unsupported Body IR type TypeVar |
| snapshots_enums_and_matching/cross_module_union | pass |  |
| snapshots_enums_and_matching/imported_enum_loop_ownership | refused | unsupported Body IR nominal type ConformanceRel |
| snapshots_enums_and_matching/unqualified_and_aliased_variant_patterns | refused | unsupported source Enum Shape on the native route |
| snapshots_functions_and_projections/cross_module_function_alias | pass |  |
| snapshots_functions_and_projections/reexport_chain | pass |  |
| snapshots_models_and_classes/imported_helper_named_like_builtin | pass |  |
| snapshots_models_and_classes/imported_private_source_class_constructor | refused | unsupported source Class generic trait adoptions, inheritance, type parameters, decorators, aliases, properties or defaults on Vault on the native route |
| snapshots_models_and_classes/imported_private_source_model_constructor | refused | unsupported Body IR nominal type PublicVault |
| snapshots_newtypes_and_serde/direct_json_trait_import_across_modules | pass |  |
| snapshots_newtypes_and_serde/facade_reexported_serde_traits | refused | unsupported Body IR model JSON serialization without serde::Serialize |
| snapshots_newtypes_and_serde/serde_module_qualified_json_traits | refused | unsupported Body IR type TypeVar |
| snapshots_newtypes_and_serde/user_module_derivables | refused | unsupported source generic Trait, supertraits, decorators, aliases or properties on the native route |
| snapshots_stdlib/loops_over_calls_returning_iterator_adopters | refused | unsupported source Model generic trait adoption on Ticks on the native route |
| snapshots_stdlib/std_io_and_fs_trait_method_calls | refused | unsupported Body IR nominal type IoError |
| snapshots_stdlib/web_route_nested_module | pass |  |
| snapshots_stdlib/web_route_private_nested_module | refused | unsupported Body IR type Json[Named[User]] |

## Wrong and driver errors

### execution_numerics_and_conversions/contextual_negative_literals_survive_direct_returns_and_same_module_ca_3 (pending, observed wrong)

```text
stdout differs
expected stdout:
  -1.0
actual stdout:
  -1

stderr:
  (empty)

```

//! Checked executable Body IR uses the same explicit typed projection as declaration rows.

use super::{Cell, NamespaceCell, Project, read_fields};
use incan_lang::errors::ErrorKind;
use incan_lang::lang::builtins::{BuiltinFnId, BuiltinMethodId};
use incan_lang::lang::surface::methods::bytes_methods::BytesMethodId;
use incan_lang::lang::surface::methods::clone_methods::CloneMethodId;
use incan_lang::lang::surface::methods::dict_methods::DictMethodId;
use incan_lang::lang::surface::methods::float_methods::FloatMethodId;
use incan_lang::lang::surface::methods::frozen_bytes_methods::FrozenBytesMethodId;
use incan_lang::lang::surface::methods::frozen_dict_methods::FrozenDictMethodId;
use incan_lang::lang::surface::methods::frozen_list_methods::FrozenListMethodId;
use incan_lang::lang::surface::methods::frozen_set_methods::FrozenSetMethodId;
use incan_lang::lang::surface::methods::iterator_methods::IteratorMethodId;
use incan_lang::lang::surface::methods::list_methods::ListMethodId;
use incan_lang::lang::surface::methods::option_methods::OptionMethodId;
use incan_lang::lang::surface::methods::result_methods::ResultMethodId;
use incan_lang::lang::surface::methods::set_methods::SetMethodId;
use incan_lang::lang::surface::methods::string_methods::StringMethodId;
use incan_lang::lang::types::numerics::{IntegerHelperFamily, IntegerHelperOperation, NumericResizeMethodId};
use incan_semantics_core::CompilerNodeId;
use incan_semantics_core::body_ir::{
    AggregateKind, ArgumentBinding, ArgumentElement, AssertionKind, BinOp, Block, Body, BodyIrModule, BoundArgument,
    CallableParam, CallableParamDefault, CallableRepresentation, CallableTarget, Callee, CheckedBridgeCall,
    CheckedBridgeOperation, CheckedForeignCall, CheckedInputParameter, CheckedInputProfile, CheckedOwnedInitializer,
    ClassInheritance, ClassMethodImplementation, ClosureBody, Constant, ConstantDeclaration, ConstructorTarget,
    DecoratedBinding, DecoratedInitialization, DefaultComputation, DictEntry, EntrypointEffect, EnumDeclaration,
    EnumVariantDeclaration, EnumVariantTarget, ExplicitInputBinding, FieldMetadata, FieldlessEnumDeclaration,
    FieldlessEnumVariantDeclaration, FieldlessEnumVariantTarget, FormatPart, FormatStyle, GeneratorBody, GlobalPlace,
    GlobalWritePolicy, HelperOp, IsInstanceRvalue, IsInstanceTarget, IterProtocol, LinkedStaticAccessors,
    LinkedStaticGuardAccessors, LoafCallable, LoafTraitCall, LocalCallableTarget, LocalDecl, LocalId, LocalOrigin,
    MatchArm, MethodPartialDefaults, MethodTarget, NamedCallableTarget, NativeCallableParameter, NativeClosureStorage,
    NativeUnionDeclaration, NewtypeDeserialize, NewtypeStringConversion, NominalDeclaration, NominalPatternTarget,
    NumericNewtypeConstant, Operand, OwnershipFact, PanicFact, PanicReason, Pattern, PatternBinding, Place, PlaceElem,
    PlaceOperand, PlaceRoot, PrimitiveTraitAdapter, ProviderActivation, ProviderActivationState,
    ProviderOperationInput, ProviderOperationPlan, RaceArm, ReflectionOperation, ResultVariant, ResultVariantKind,
    ReturnedView, Rvalue, ScopeId, ScopeInfo, SpreadElement, SpreadKind, Statement, StatementKind, StaticDeclaration,
    StaticInitializer, StringConversionContract, StringConversionSelection, TraitAdoption, TraitImplementation,
    TraitTypeArgument, TryErrorRouting, TypeAliasDeclaration, TypeMetadataOperation, TypeMetadataTarget,
    TypedNumericConstant, UnOp, UnsafeAcknowledgment, ValidatedConstructor, ValueEnumBacking, ValueEnumDeclaration,
    ValueEnumVariantDeclaration, ValueEnumVariantTarget,
};
use incan_semantics_core::record_identity::RecordPackageIdentity;
use incan_semantics_core::{AbiV0RuntimeRequirement, CompilerNodeKind};

macro_rules! model_with_omissions {
    ($ty:ty { $($field:ident : $id:literal),* $(,)? } default [$($omitted:ident),+]) => {
        impl Project for $ty {
            /// Project every persisted field while ignoring the named invocation-only fields.
            fn project(&self) -> Cell {
                let Self { $($field),*, $($omitted: _),+ } = self;
                Cell::Record(vec![$(($id, $field.project())),*])
            }

            /// Restore the persisted fields and initialize the named invocation-only fields to their defaults.
            fn restore(value: Cell) -> Result<Self, String> {
                let Cell::Record(fields) = value else {
                    return Err(concat!("expected checked executable ", stringify!($ty)).into());
                };
                let [$($field),*] = read_fields(fields, [$($id),*])?;
                Ok(Self { $($field: Project::restore($field)?),*, $($omitted: Default::default()),+ })
            }
        }
    };
}

macro_rules! tuple_record {
    ($ty:ident, $id:literal) => {
        impl Project for $ty {
            /// Keep a nominal wrapper distinct from its sole field's storage type.
            fn project(&self) -> Cell {
                Cell::Record(vec![($id, self.0.project())])
            }

            /// Restore the tagged wrapper from its one exact field.
            fn restore(value: Cell) -> Result<Self, String> {
                let Cell::Record(fields) = value else {
                    return Err(concat!("expected checked executable ", stringify!($ty)).into());
                };
                let [inner] = read_fields(fields, [$id])?;
                Ok(Self(Project::restore(inner)?))
            }
        }
    };
}

impl Project for CompilerNodeId {
    /// Preserve the node kind and complete semantic path, including generated anchors.
    fn project(&self) -> Cell {
        Cell::Record(vec![
            (0x0b27c158feb3, self.kind().project()),
            (0xa0af9f865bf6, Cell::Text(self.path().to_owned())),
        ])
    }

    /// Recreate the original node identity without inferring its kind from path spelling.
    fn restore(value: Cell) -> Result<Self, String> {
        let Cell::Record(fields) = value else {
            return Err("expected checked executable node identity".into());
        };
        let [kind, path] = read_fields(fields, [0x0b27c158feb3, 0xa0af9f865bf6])?;
        Ok(Self::new(Project::restore(kind)?, String::restore(path)?))
    }
}

include!("executable_projection.rs");

/// Project one checked body under the shared graph Body IR contract.
pub fn executable_body_value(body: &Body) -> NamespaceCell {
    body.project()
}

/// Restore one admitted executable body before binding its selected physical provenance.
pub fn restore_executable_body_value(value: NamespaceCell) -> Result<Body, String> {
    Body::restore(value)
}

/// Project the completed module's executable fields under the graph Body IR contract.
pub fn executable_module_value(module: &BodyIrModule) -> NamespaceCell {
    module.project()
}

/// Restore an admitted checked module with fresh defaults for invocation-only state.
pub fn restore_executable_module_value(value: NamespaceCell) -> Result<BodyIrModule, String> {
    BodyIrModule::restore(value)
}

/// Check the graph-only body and module round trips against actual checked facts and portable admission.
#[cfg(test)]
pub(crate) fn verify_executable_codec(mut expected: BodyIrModule) -> Result<(), String> {
    use super::{to_atom, to_cell};
    let adoption = expected
        .nominal_declarations
        .iter()
        .find_map(|nominal| nominal.trait_adoptions.first())
        .ok_or("missing checked adoption")?;
    assert_eq!(adoption.fields.len(), 1);
    let bytes = internal_models::encode_executable_module(&to_atom(executable_module_value(&expected)))?;
    assert!(bytes.starts_with(b"CRMB"));
    let reopened = restore_executable_module_value(to_cell(internal_models::decode_executable_module(&bytes)?)?)?;
    let reopened_adoption = reopened
        .nominal_declarations
        .iter()
        .find_map(|nominal| nominal.trait_adoptions.first())
        .ok_or("missing restored adoption")?;
    assert_eq!(reopened_adoption.fields, adoption.fields);
    expected.published_callables.clear();
    assert_eq!(reopened, expected);
    assert_eq!(
        internal_models::encode_executable_module(&to_atom(executable_module_value(&reopened)))?,
        bytes
    );
    assert!(internal_models::decode_executable_body(&bytes).is_err());
    let body = expected
        .bodies
        .first()
        .ok_or("missing checked body in executable codec fixture")?;
    let owner = internal_models::encode_executable_body(&to_atom(executable_body_value(body)))?;
    assert_eq!(
        restore_executable_body_value(to_cell(internal_models::decode_executable_body(&owner)?)?)?,
        *body
    );
    assert!(internal_models::decode_executable_module(&owner).is_err());
    let package =
        incan_semantics_core::executable_representation::encode_module(&expected).map_err(|error| error.to_string())?;
    assert!(internal_models::decode_executable_module(&package).is_err());
    Ok(())
}

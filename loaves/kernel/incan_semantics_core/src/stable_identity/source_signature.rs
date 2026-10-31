//! Position-independent source signatures, before executable carrier selection.

use std::collections::BTreeMap;

use serde::Serialize;

use super::{DeclarationSignature, StableDeclarationId};
use crate::facts::CanonicalSymbolId;
use crate::types::{IncanCallableParam, IncanCallableParamKind, IncanType};

/// Versioned identity profile shared by declaration producers and codegraph consumers.
pub const SOURCE_SIGNATURE_PROFILE: &str = "incan.source-signature/1";

/// Checked source types retain nominal declaration authority separately from structural spelling.
#[derive(Serialize)]
enum SourceType {
    /// A checked nominal reference, excluding its diagnostic coordinates.
    Nominal(StableDeclarationId),
    /// Position in the callable's checked owner-then-callable binder frame.
    Parameter(usize),
    /// A structural constructor applied to checked source arguments.
    Apply(Box<SourceType>, Vec<SourceType>),
    /// Source callable shape without argument names.
    Callable(Vec<SourceParameter>, Box<SourceType>),
    /// Ordered tuple elements.
    Tuple(Vec<SourceType>),
    /// Distinct structural wrappers around one checked source type.
    Wrapped(&'static str, Box<SourceType>),
    /// Fixed-size native array; the evaluated length is semantic.
    Array(Box<SourceType>, u64),
    /// Foreign pointer storage, including mutability.
    Pointer(bool, Box<SourceType>),
    /// A non-composite builtin or exact foreign type from the shared type substrate.
    Atom(IncanType),
}

/// Source parameter contract without a binding name or executable ownership decision.
#[derive(Serialize)]
struct SourceParameter {
    /// Fixed or rest argument shape.
    kind: IncanCallableParamKind,
    /// Whether callers may omit this parameter.
    has_default: bool,
    /// Whether the source callable exposes mutation through this parameter.
    is_mut: bool,
    /// Checked source type, with nominal provenance preserved.
    ty: SourceType,
}

/// Inputs already resolved by the checker, never a resolver over raw source annotations.
pub struct SourceSignatureContext<'a> {
    /// The declaration's own checked nominal lookup environment, including aliases and imported owners.
    pub nominals: &'a BTreeMap<String, CanonicalSymbolId>,
    /// Checked type parameters in lexical order; inner binders shadow outer ones.
    pub binders: &'a [String],
}

impl SourceSignatureContext<'_> {
    /// Project a callable's source signature, returning no identity for an unresolved source type.
    pub fn callable(
        &self,
        parameters: &[IncanCallableParam],
        return_type: &IncanType,
        is_async: bool,
    ) -> Option<DeclarationSignature> {
        let parameters = self.parameters(parameters)?;
        let result = self.ty(return_type)?;
        let digest = crate::semantic_digest::semantic_digest(&(
            SOURCE_SIGNATURE_PROFILE,
            self.binders.len(),
            is_async,
            parameters,
            result,
        ))
        .ok()?;
        Some(DeclarationSignature(format!("{SOURCE_SIGNATURE_PROFILE}:{digest}")))
    }

    /// Preserve callable parameter kinds and optionality without naming the source bindings.
    fn parameters(&self, parameters: &[IncanCallableParam]) -> Option<Vec<SourceParameter>> {
        parameters
            .iter()
            .map(|parameter| {
                Some(SourceParameter {
                    kind: parameter.kind,
                    has_default: parameter.has_default,
                    is_mut: parameter.is_mut,
                    ty: self.ty(&parameter.ty)?,
                })
            })
            .collect()
    }

    /// Read a checked nominal's edit-stable identity. Body semantic projections use this same boundary for native
    /// carrier keys; callers must supply checked bindings rather than infer authority from the key's bytes.
    pub(crate) fn nominal_identity(&self, name: &str) -> Option<StableDeclarationId> {
        StableDeclarationId::from_canonical(self.nominals.get(name)?, None, None)
    }

    /// Resolve nominal references through checked identities; never infer one by parsing a carrier name.
    fn named(&self, name: &str) -> Option<SourceType> {
        if self.nominals.contains_key(name) {
            return self.nominal_identity(name).map(SourceType::Nominal);
        }
        // Builtin structural constructors have no source declaration. Arbitrary unknown names fail closed.
        if incan_lang::lang::types::collections::from_str(name).is_some()
            || name == incan_lang::lang::types::UNION_TYPE_NAME
        {
            return Some(SourceType::Atom(IncanType::Named(name.to_string())));
        }
        None
    }

    /// Recursively project checked source shape without invoking Body IR's representation rewrite.
    fn ty(&self, ty: &IncanType) -> Option<SourceType> {
        Some(match ty {
            IncanType::Named(name) => return self.named(name),
            IncanType::TypeVar(name) => SourceType::Parameter(self.binders.iter().rposition(|binder| binder == name)?),
            IncanType::Generic { base, args } => SourceType::Apply(
                Box::new(self.named(base)?),
                args.iter().map(|argument| self.ty(argument)).collect::<Option<_>>()?,
            ),
            IncanType::Function { params, return_type } => {
                SourceType::Callable(self.parameters(params)?, Box::new(self.ty(return_type)?))
            }
            IncanType::Tuple(elements) => {
                SourceType::Tuple(elements.iter().map(|element| self.ty(element)).collect::<Option<_>>()?)
            }
            IncanType::Ref(inner) => SourceType::Wrapped("ref", Box::new(self.ty(inner)?)),
            IncanType::RefMut(inner) => SourceType::Wrapped("ref_mut", Box::new(self.ty(inner)?)),
            IncanType::TypeToken(inner) => SourceType::Wrapped("type_token", Box::new(self.ty(inner)?)),
            IncanType::NativeArray { element, length } => SourceType::Array(Box::new(self.ty(element)?), *length),
            IncanType::NativePointer { mutable, pointee } => SourceType::Pointer(*mutable, Box::new(self.ty(pointee)?)),
            IncanType::Infer | IncanType::Unknown | IncanType::Unsupported(_) => return None,
            atom => SourceType::Atom(atom.clone()),
        })
    }
}

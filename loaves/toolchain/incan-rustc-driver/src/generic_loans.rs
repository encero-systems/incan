//! Retain source type-parameter loans through the canonical native types selected by specialization.

use std::collections::HashMap;
use std::ops::ControlFlow;

use crate::error::PlanError;
use crate::plan::GenericLoan;
use rustc_middle::ty::{self, Ty, TyCtxt, TypeFoldable, TypeFolder, TypeSuperFoldable, TypeVisitable, TypeVisitor};

/// Bind only the native regions at retained source type-parameter occurrences. Equal native carriers do not imply
/// equal loans: the source parameter identity and each region's structural position select its binder.
pub(crate) fn signature<'tcx>(
    tcx: TyCtxt<'tcx>,
    signature: ty::PolyFnSig<'tcx>,
    sites: &[GenericLoan],
) -> Result<ty::PolyFnSig<'tcx>, PlanError> {
    if sites.is_empty() {
        return Ok(signature);
    }
    let raw = signature.skip_binder();
    let mut types = raw.inputs_and_output.to_vec();
    let mut loans = Loans {
        tcx,
        bound: signature.bound_vars().to_vec(),
        variables: HashMap::new(),
        parameter: 0,
        ordinal: 0,
    };
    for site in sites {
        let slot = usize::try_from(site.slot).map_err(|_| invalid("negative generic loan slot"))?;
        let parameter = usize::try_from(site.parameter).map_err(|_| invalid("negative generic loan parameter"))?;
        let target = types
            .get_mut(slot)
            .ok_or_else(|| invalid("generic loan slot exceeds its signature"))?;
        let expected = crate::types::native_type(tcx, &site.ty)?;
        if expected.visit_with(&mut ErasedLoans).is_continue() {
            continue;
        }
        loans.parameter = parameter;
        loans.ordinal = 0;
        let replacement = replace(*target, &site.path, expected, &mut loans);
        *target = match replacement {
            Ok(value) => value,
            Err(error) if slot < raw.inputs().len() => match target.kind() {
                ty::Ref(region, inner, mutable) => {
                    loans.ordinal = 0;
                    let inner = replace(*inner, &site.path, expected, &mut loans)?;
                    Ty::new_ref(tcx, *region, inner, *mutable)
                }
                _ => return Err(error),
            },
            Err(error) => return Err(error),
        };
    }
    Ok(ty::Binder::bind_with_vars(
        ty::FnSig {
            inputs_and_output: tcx.mk_type_list(&types),
            ..raw
        },
        tcx.mk_bound_variable_kinds(&loans.bound),
    ))
}

/// Replace one retained subtree after verifying its concrete storage against the selected type argument.
fn replace<'tcx>(
    value: Ty<'tcx>,
    path: &[i64],
    expected: Ty<'tcx>,
    loans: &mut Loans<'tcx>,
) -> Result<Ty<'tcx>, PlanError> {
    let Some((&head, tail)) = path.split_first() else {
        if loans.tcx.erase_and_anonymize_regions(value) != loans.tcx.erase_and_anonymize_regions(expected) {
            return Err(invalid(
                "generic loan occurrence differs from its selected type argument",
            ));
        }
        return Ok(value.fold_with(loans));
    };
    let index = usize::try_from(head).map_err(|_| invalid("negative generic loan projection"))?;
    match value.kind() {
        ty::Ref(region, inner, mutable) if index == 0 => {
            let inner = replace(*inner, tail, expected, loans)?;
            Ok(Ty::new_ref(loans.tcx, *region, inner, *mutable))
        }
        ty::Slice(inner) if index == 0 => {
            let inner = replace(*inner, tail, expected, loans)?;
            Ok(Ty::new_slice(loans.tcx, inner))
        }
        ty::Tuple(elements) => {
            let mut elements = elements.to_vec();
            let target = elements
                .get_mut(index)
                .ok_or_else(|| invalid("generic loan tuple projection exceeds arity"))?;
            *target = replace(*target, tail, expected, loans)?;
            Ok(Ty::new_tup(loans.tcx, &elements))
        }
        ty::Adt(definition, arguments) => {
            let mut arguments = arguments.to_vec();
            let target = arguments
                .iter_mut()
                .filter(|argument| argument.as_type().is_some())
                .nth(index)
                .ok_or_else(|| invalid("generic loan type projection exceeds arity"))?;
            let inner = target
                .as_type()
                .ok_or_else(|| invalid("generic loan projection is not a type"))?;
            *target = replace(inner, tail, expected, loans)?.into();
            Ok(Ty::new_adt(loans.tcx, *definition, loans.tcx.mk_args(&arguments)))
        }
        _ => Err(invalid("generic loan projection has no native type subtree")),
    }
}

/// Report malformed provenance as a native-plan refusal rather than inventing a lifetime relationship.
fn invalid(reason: &str) -> PlanError {
    PlanError::Invalid {
        function: "generic signature loan".into(),
        reason: reason.into(),
    }
}

/// Recognize erased native ADT lifetime arguments. Explicit references already use their retained signature
/// contract; restoring them again would confuse source projections such as bytes with a native byte slice.
struct ErasedLoans;

impl<'tcx> TypeVisitor<TyCtxt<'tcx>> for ErasedLoans {
    type Result = ControlFlow<()>;

    /// Function-pointer binders retain their own input contract rather than borrowing the surrounding value.
    fn visit_ty(&mut self, value: Ty<'tcx>) -> Self::Result {
        if matches!(value.kind(), ty::FnPtr(..)) {
            return ControlFlow::Continue(());
        }
        if let ty::Adt(_, arguments) = value.kind()
            && arguments
                .iter()
                .any(|argument| argument.as_region().is_some_and(|region| region.is_erased()))
        {
            return ControlFlow::Break(());
        }
        rustc_middle::ty::TypeSuperVisitable::super_visit_with(&value, self)
    }
}

/// One signature's source-parameter binders; each occurrence restarts its structural region ordinal.
struct Loans<'tcx> {
    tcx: TyCtxt<'tcx>,
    bound: Vec<ty::BoundVariableKind<'tcx>>,
    variables: HashMap<(usize, usize), ty::Region<'tcx>>,
    parameter: usize,
    ordinal: usize,
}

impl<'tcx> TypeFolder<TyCtxt<'tcx>> for Loans<'tcx> {
    /// Supply the native type interner.
    fn cx(&self) -> TyCtxt<'tcx> {
        self.tcx
    }

    /// A callable's independently bound regions never become loans of its surrounding generic value.
    fn fold_binder<T: TypeFoldable<TyCtxt<'tcx>>>(&mut self, value: ty::Binder<'tcx, T>) -> ty::Binder<'tcx, T> {
        value
    }

    /// Leave function-pointer binders to their own signature's input contract.
    fn fold_ty(&mut self, value: Ty<'tcx>) -> Ty<'tcx> {
        if matches!(value.kind(), ty::FnPtr(..)) {
            return value;
        }
        value.super_fold_with(self)
    }

    /// Restore only metadata-erased regions under the same source type parameter and structural position.
    fn fold_region(&mut self, region: ty::Region<'tcx>) -> ty::Region<'tcx> {
        let key = (self.parameter, self.ordinal);
        self.ordinal += 1;
        if !region.is_erased() {
            self.variables.entry(key).or_insert(region);
            return region;
        }
        *self.variables.entry(key).or_insert_with(|| {
            let var = ty::BoundVar::from_usize(self.bound.len());
            self.bound
                .push(ty::BoundVariableKind::Region(ty::BoundRegionKind::Anon));
            ty::Region::new_bound(
                self.tcx,
                ty::INNERMOST,
                ty::BoundRegion {
                    var,
                    kind: ty::BoundRegionKind::Anon,
                },
            )
        })
    }
}

//! Spike step 11: real Body IR, lowered to MIR, compiled natively.
//!
//! The driver runs the Incan front end in-process (DD-0004): it lexes, parses and type-checks `step11_kernels.incn`
//! (the unchanged kernels of the `fib` and `collatz` benchmarks), builds Body IR with the compiler's own
//! `build_body_ir_module_v0`, injects each body's declaration as a rustc item, and supplies each body as MIR lowered
//! from that Body IR. The Incan unit has an empty crate root. `step11_app.rs`, plain Rust, calls the kernels.
//!
//! The lowering covers the subset these kernels use and refuses everything else by name: `int`/`bool` locals,
//! constants, copies and moves, `+ - *` (overflow-checked when rustc's overflow checks are on), comparisons, `not`,
//! Python-semantics `//` and `%` through the same `incan_std_core::num` helpers the emitted-Rust route calls, `if`,
//! `loop`/`break`/`continue`, `return`, calls to the unit's own functions, and `for` over a builtin `range`, which is
//! lowered as Rust lowers a `for` loop: a `core::ops::Range<i64>` polled with `Iterator::next`.
//!
//! This prototype is Rust because it is evidence of the mapping; the product lowering is Incan (DD-0004).
#![feature(rustc_private)]
extern crate rustc_abi;
extern crate rustc_apfloat;
extern crate rustc_ast;
extern crate rustc_data_structures;
extern crate rustc_driver;
extern crate rustc_hir;
extern crate rustc_index;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_session;
extern crate rustc_span;
extern crate thin_vec;

mod common;

use common::{Cfg, extern_crate, extern_item, function, local_item, model, public, t};
use incan_frontend::body_ir::build_body_ir_module_v0;
use incan_frontend::{lexer, parser, typechecker::TypeChecker};
use incan_lang::lang::builtins::BuiltinFnId;
use incan_semantics_core::body_ir as bir;
use incan_semantics_core::{IncanPrimitiveType, IncanType};
use rustc_abi::{FieldIdx, VariantIdx};
use rustc_ast as ast;
use rustc_data_structures::steal::Steal;
use rustc_hir::def_id::LocalDefId;
use rustc_index::IndexVec;
use rustc_middle::mir::interpret::Scalar;
use rustc_middle::mir::*;
use rustc_middle::ty::{self, Ty, TyCtxt};
use rustc_span::{BytePos, FileName, Span};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

/// The checked unit, shared with `mir_built`: query providers are plain function pointers.
static MODULE: OnceLock<bir::BodyIrModule> = OnceLock::new();
/// Where the `.incn` file starts in rustc's source map, so Body IR spans become rustc spans.
static SOURCE_START: OnceLock<BytePos> = OnceLock::new();
/// The models this unit declares, which are the only nominal types the lowering maps to Rust structs.
static MODELS: OnceLock<std::collections::BTreeSet<String>> = OnceLock::new();

fn is_unit_model(name: &str) -> bool {
    MODELS.get().is_some_and(|models| models.contains(name))
}

// ============================================================================
// Front end: the real Incan checker and Body IR builder, in-process
// ============================================================================

fn check_and_build(source: &str) -> Result<(bir::BodyIrModule, Vec<String>), String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lexing failed: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("parsing failed: {errors:?}"))?;
    let mut checker = TypeChecker::new();
    let module_path = vec!["kernels".to_string()];
    checker.set_current_module_path(Some(module_path.clone()));
    checker.check_program(&program).map_err(|errors| format!("checking failed: {errors:?}"))?;
    // Body IR gives a decorated function its source body, but a decorator can replace what the function does:
    // `@rust.extern` delegates to a Rust function and its `...` body is only a placeholder. Body IR does not record
    // that, so lowering such a body would silently compile the placeholder. Refuse every decorated function until
    // the checked facts say what the decorator means.
    for declaration in &program.declarations {
        if let incan_frontend::ast::Declaration::Function(function) = &declaration.node
            && !function.decorators.is_empty()
        {
            return Err(format!("native lowering of `{}` does not support decorated functions (such as `@rust.extern`) yet", function.name));
        }
    }
    // A `mut` parameter writes through to the caller (RFC 129), but Body IR passes its argument as a copy (#2022).
    // Lowering that copy would compile a program that silently mutates a throwaway value, so refuse it by name.
    let mut_parameter = |params: &[incan_frontend::ast::Spanned<incan_frontend::ast::Param>]| params.iter().any(|param| param.node.is_mut);
    for declaration in &program.declarations {
        let offending = match &declaration.node {
            incan_frontend::ast::Declaration::Function(function) if mut_parameter(&function.params) => Some(function.name.clone()),
            incan_frontend::ast::Declaration::Model(model_decl) => model_decl.methods.iter().find(|m| mut_parameter(&m.node.params)).map(|m| m.node.name.clone()),
            _ => None,
        };
        if let Some(name) = offending {
            return Err(format!("native lowering of `{name}` does not support `mut` parameters yet (#2022)"));
        }
    }
    // A model's decorators (`@derive`), adopted traits, aliases, partials and properties all change what the model
    // means natively, and none of them is in Body IR's nominal declarations yet.
    let mut fieldless_models = Vec::new();
    for declaration in &program.declarations {
        if let incan_frontend::ast::Declaration::Model(model_decl) = &declaration.node {
            let extras = !model_decl.decorators.is_empty() || !model_decl.traits.is_empty() || !model_decl.method_aliases.is_empty()
                || !model_decl.method_partials.is_empty() || !model_decl.properties.is_empty() || !model_decl.type_params.is_empty();
            if extras {
                return Err(format!("native lowering of `{}` does not support model decorators, traits, aliases, partials, properties or type parameters yet", model_decl.name));
            }
            if model_decl.fields.is_empty() {
                fieldless_models.push(model_decl.name.to_string());
            }
        }
    }
    let module = build_body_ir_module_v0(&program, &module_path, checker.type_info());
    let mut names = std::collections::BTreeSet::new();
    for body in &module.bodies {
        if !names.insert((receiver_owner(body), body.name.as_str())) {
            return Err(format!("native lowering of `{}` does not support overloaded functions yet", body.name));
        }
    }
    // Body IR declares only the models its first consumer could run; a field-less model with methods is not among
    // them. Its Rust struct needs nothing Body IR would supply, so the driver declares it from the source.
    fieldless_models.retain(|name| !module.nominal_declarations.iter().any(|declared| &declared.name == name));
    Ok((module, fieldless_models))
}

/// The model a method belongs to: its receiver's type. `None` for a free function.
fn receiver_owner(body: &bir::Body) -> Option<&str> {
    body.locals.iter().find_map(|local| match (&local.origin, &local.ty) {
        (bir::LocalOrigin::Receiver { .. }, IncanType::Named(owner)) => Some(owner.as_str()),
        _ => None,
    })
}

/// Whether a method's receiver is `mut self`.
fn receiver_is_mutable(body: &bir::Body) -> bool {
    body.locals.iter().any(|local| matches!(local.origin, bir::LocalOrigin::Receiver { mutable: true }))
}

/// A short, stable label for an Incan type in a refusal, so refusals group by kind of type.
fn type_label(ty: &IncanType) -> String {
    match ty {
        IncanType::Generic { base, .. } => format!("{base}[..]"),
        IncanType::Named(name) if is_unit_model(name) => name.clone(),
        IncanType::Named(_) => "a class, enum, newtype or imported type".to_string(),
        IncanType::Primitive(primitive) => format!("{primitive:?}"),
        other => format!("{other:?}").split(['(', ' ', '{']).next().unwrap_or("?").to_string(),
    }
}

/// The rustc spelling of an Incan type this lowering supports, as the declarations the driver injects name it.
fn type_spec(ty: &IncanType) -> Option<common::TySpec> {
    // `List[T]` is `Vec<T>` (RFC 121).
    if let IncanType::Generic { base, args } = ty
        && base == "List"
        && let [element] = args.as_slice()
    {
        return Some(common::TySpec("Vec".to_string(), vec![type_spec(element)?]));
    }
    let name = match ty {
        IncanType::Primitive(IncanPrimitiveType::Int) => "i64",
        IncanType::Primitive(IncanPrimitiveType::Bool) => "bool",
        IncanType::Primitive(IncanPrimitiveType::Float) => "f64",
        IncanType::Primitive(IncanPrimitiveType::Unit) => "()",
        IncanType::Primitive(IncanPrimitiveType::Str) => "String",
        // A model is the Rust struct of the same name the driver declares for it (RFC 121).
        IncanType::Named(name) if is_unit_model(name) => name.as_str(),
        _ => return None,
    };
    Some(t(name))
}

fn mir_ty<'tcx>(tcx: TyCtxt<'tcx>, ty: &IncanType) -> Option<Ty<'tcx>> {
    match ty {
        IncanType::Primitive(IncanPrimitiveType::Int) => Some(tcx.types.i64),
        IncanType::Primitive(IncanPrimitiveType::Bool) => Some(tcx.types.bool),
        IncanType::Primitive(IncanPrimitiveType::Float) => Some(tcx.types.f64),
        IncanType::Primitive(IncanPrimitiveType::Unit) => Some(tcx.types.unit),
        IncanType::Primitive(IncanPrimitiveType::Str) => Some(string_ty(tcx)),
        IncanType::Named(name) if is_unit_model(name) => Some(tcx.type_of(local_item(tcx, name)).instantiate_identity().skip_normalization()),
        IncanType::Generic { base, args } if base == "List" => match args.as_slice() {
            [element] => Some(vec_ty(tcx, mir_ty(tcx, element)?)),
            _ => None,
        },
        _ => None,
    }
}

/// The `Vec` item.
fn vec_def(tcx: TyCtxt<'_>) -> rustc_hir::def_id::DefId {
    tcx.get_diagnostic_item(rustc_span::Symbol::intern("Vec")).unwrap_or_else(|| tcx.dcx().fatal("this sysroot does not name `Vec`"))
}

/// `Vec<element>` with its default allocator: the type `Vec::<element>::new()` returns.
fn vec_ty<'tcx>(tcx: TyCtxt<'tcx>, element: Ty<'tcx>) -> Ty<'tcx> {
    let new = common::inherent_method(tcx, vec_def(tcx), "new");
    tcx.fn_sig(new).instantiate(tcx, &[element.into()]).skip_normalization().skip_binder().output()
}

/// Incan's `str` is Rust's `String`, the representation RFC 121 gives it.
fn string_ty<'tcx>(tcx: TyCtxt<'tcx>) -> Ty<'tcx> {
    let Some(string) = tcx.lang_items().string() else { tcx.dcx().fatal("this sysroot has no `String` lang item") };
    tcx.type_of(string).instantiate_identity().skip_normalization()
}

// ============================================================================
// Lowering: structured Body IR to a MIR control-flow graph
// ============================================================================

struct Lowering<'a, 'tcx> {
    tcx: TyCtxt<'tcx>,
    body: &'a bir::Body,
    cfg: Cfg<'tcx>,
    /// The MIR place standing for each Body IR local. A borrow of a range aliases the range itself.
    places: HashMap<u32, Place<'tcx>>,
    /// The block statements are currently appended to.
    current: BasicBlock,
    /// `(continue target, break target)` for each enclosing `loop`, innermost last.
    loops: Vec<(BasicBlock, BasicBlock)>,
    exit: BasicBlock,
    /// Body IR locals that hold a borrow of a list a `for` loop polls; natively each is a `core::slice::Iter`.
    list_iters: std::collections::HashSet<u32>,
}

impl<'a, 'tcx> Lowering<'a, 'tcx> {
    fn refuse(&self, what: &str) -> ! {
        self.tcx.dcx().fatal(format!("native lowering of `{}` does not support {what} yet", self.body.name))
    }

    fn span(&self, span: &incan_semantics_core::HirSourceSpan) -> Span {
        let Some(start) = SOURCE_START.get().copied() else { return self.cfg.span };
        let (lo, hi) = (u32::try_from(span.start), u32::try_from(span.end));
        match (lo, hi) {
            (Ok(lo), Ok(hi)) => Span::with_root_ctxt(start + BytePos(lo), start + BytePos(hi)),
            _ => self.cfg.span,
        }
    }

    /// Parameters map to MIR's argument locals; every other local gets a fresh MIR local of its MIR type, except
    /// ranges and borrows of ranges, which `plan_ranges` has already placed.
    fn plan_locals(&mut self) {
        for (index, param) in self.body.param_locals.iter().enumerate() {
            self.places.insert(param.0, self.cfg.arg(index as u32));
        }
        self.plan_ranges();
        self.plan_list_iterators();
        for local in &self.body.locals {
            if self.places.contains_key(&local.id.0) {
                continue;
            }
            let Some(ty) = mir_ty(self.tcx, &local.ty) else { self.refuse(&format!("a local of type `{:?}`", local.ty)) };
            let place = self.cfg.temp(ty);
            self.places.insert(local.id.0, place);
        }
    }

    /// Body IR records a builtin `range(a, b)` value as `List[int]`, and the borrow the `for` loop polls with the same
    /// type. Natively both are one `core::ops::Range<i64>`, the type Rust's own `a..b` has.
    fn plan_ranges(&mut self) {
        let mut ranges = Vec::new();
        visit_statements(&self.body.block, &mut |stmt| match &stmt.kind {
            bir::StatementKind::Call { destination: Some(dest), callee, .. } if is_builtin_range(callee) => ranges.extend(local_of(dest)),
            _ => {}
        });
        let range_ty = self.range_ty();
        for range in &ranges {
            let place = self.cfg.temp(range_ty);
            self.places.insert(*range, place);
        }
        visit_statements(&self.body.block, &mut |stmt| {
            if let bir::StatementKind::Assign { place, rvalue: bir::Rvalue::Use(bir::Operand::Place(source)) } = &stmt.kind
                && source.fact == bir::OwnershipFact::Borrow
                && let (Some(alias), Some(target)) = (local_of(place), local_of(&source.place))
                && ranges.contains(&target)
            {
                ranges.push(alias);
            }
        });
        let shared: Vec<(u32, Place<'tcx>)> = ranges.iter().filter_map(|r| self.alias_target(*r).map(|p| (*r, p))).collect();
        self.places.extend(shared);
    }

    /// A `for` loop over a list polls a borrow of the list. Natively that borrow is the `core::slice::Iter<T>` that
    /// `IntoIterator::into_iter(&list)` returns, the iterator Rust's own `for item in &list` uses. Only lists of `int`,
    /// `float` and `bool` are iterated for now: their items are copied out of the `&T` each poll yields.
    fn plan_list_iterators(&mut self) {
        let mut polled = std::collections::HashSet::new();
        visit_statements(&self.body.block, &mut |stmt| {
            if let bir::StatementKind::IterNext { iterator: bir::Operand::Place(source), .. } = &stmt.kind {
                polled.extend(local_of(&source.place));
            }
        });
        let mut iterators = Vec::new();
        visit_statements(&self.body.block, &mut |stmt| {
            if let bir::StatementKind::Assign { place, rvalue: bir::Rvalue::Use(bir::Operand::Place(source)) } = &stmt.kind
                && source.fact == bir::OwnershipFact::Borrow
                && let (Some(iterator), Some(list)) = (local_of(place), local_of(&source.place))
                && polled.contains(&iterator)
            {
                iterators.push((iterator, list));
            }
        });
        let tcx = self.tcx;
        for (iterator, list) in iterators {
            // Body IR types a `range(..)` value as `List[int]` too; ranges and their borrows are already planned.
            let is_range = |place: Option<&Place<'tcx>>| place.is_some_and(|p| matches!(p.ty(&self.cfg.locals, tcx).ty.kind(), ty::Adt(adt, _) if Some(adt.did()) == tcx.lang_items().range_struct()));
            if is_range(self.places.get(&iterator)) || is_range(self.places.get(&list)) {
                continue;
            }
            let element = self.body.locals.iter().find(|l| l.id.0 == list).and_then(|l| match &l.ty {
                IncanType::Generic { base, args } if base == "List" => args.first(),
                _ => None,
            });
            let Some(element) = element else { continue };
            if !matches!(element, IncanType::Primitive(IncanPrimitiveType::Int | IncanPrimitiveType::Float | IncanPrimitiveType::Bool)) {
                self.refuse("iterating a list of non-scalar items");
            }
            let Some(element) = mir_ty(tcx, element) else { continue };
            let Some(slice_iter) = tcx.get_diagnostic_item(rustc_span::Symbol::intern("SliceIter")) else { self.refuse("list iteration without `core::slice::Iter`") };
            let iter_ty = Ty::new_adt(tcx, tcx.adt_def(slice_iter), tcx.mk_args(&[tcx.lifetimes.re_erased.into(), element.into()]));
            let place = self.cfg.temp(iter_ty);
            self.places.insert(iterator, place);
            self.list_iters.insert(iterator);
        }
    }

    /// `iterator = IntoIterator::into_iter(&list)`.
    fn lower_list_iterator(&mut self, list: &bir::Place, dest: Place<'tcx>, span: Span) {
        let tcx = self.tcx;
        let mut list = self.place(list);
        while let ty::Ref(..) = list.ty(&self.cfg.locals, tcx).ty.kind() {
            list = tcx.mk_place_deref(list);
        }
        let list_ty = list.ty(&self.cfg.locals, tcx).ty;
        let erased = tcx.lifetimes.re_erased;
        let list_ref_ty = Ty::new_imm_ref(tcx, erased, list_ty);
        let list_ref = self.cfg.temp(list_ref_ty);
        self.cfg.assign(self.current, list_ref, Rvalue::Ref(erased, BorrowKind::Shared, list), span);
        let Some(into_iter) = tcx.lang_items().into_iter_fn() else { self.refuse("iteration without `IntoIterator::into_iter`") };
        self.call(into_iter, &[list_ref_ty.into()], vec![Operand::Move(list_ref)], dest, span);
    }

    fn alias_target(&self, local: u32) -> Option<Place<'tcx>> {
        if let Some(place) = self.places.get(&local) {
            return Some(*place);
        }
        let mut found = None;
        visit_statements(&self.body.block, &mut |stmt| {
            if let bir::StatementKind::Assign { place, rvalue: bir::Rvalue::Use(bir::Operand::Place(source)) } = &stmt.kind
                && local_of(place) == Some(local)
            {
                found = local_of(&source.place).and_then(|target| self.places.get(&target).copied());
            }
        });
        found
    }

    fn range_ty(&self) -> Ty<'tcx> {
        let Some(range) = self.tcx.lang_items().range_struct() else { self.refuse("ranges without the `Range` lang item") };
        Ty::new_adt(self.tcx, self.tcx.adt_def(range), self.tcx.mk_args(&[self.tcx.types.i64.into()]))
    }

    /// A place to read.
    fn place(&mut self, place: &bir::Place) -> Place<'tcx> {
        self.place_for(place, false)
    }

    /// A place to write: an index projection then goes through `list_get_mut`.
    fn place_to_write(&mut self, place: &bir::Place) -> Place<'tcx> {
        self.place_for(place, true)
    }

    fn place_for(&mut self, place: &bir::Place, write: bool) -> Place<'tcx> {
        let bir::PlaceRoot::Local(root) = &place.root else { self.refuse("global places") };
        let mut current = self.places.get(&root.0).copied().unwrap_or_else(|| self.refuse("an unplanned local"));
        for elem in &place.projection {
            match elem {
                bir::PlaceElem::Field { name, .. } => current = self.field_place(current, name),
                bir::PlaceElem::Index(index) => current = self.list_element(current, index, write),
                bir::PlaceElem::Slice { .. } => self.refuse("slices"),
            }
        }
        current
    }

    /// `list[index]`: `*incan_std_core::collections::list_get(list.as_slice(), index)`, or `list_get_mut` on
    /// `as_mut_slice()` to write, as the emitted route indexes, with its Python-style negative indices and bounds panic.
    fn list_element(&mut self, list: Place<'tcx>, index: &bir::Operand, write: bool) -> Place<'tcx> {
        let tcx = self.tcx;
        let span = self.cfg.span;
        let mut list = list;
        while let ty::Ref(..) = list.ty(&self.cfg.locals, tcx).ty.kind() {
            list = tcx.mk_place_deref(list);
        }
        let list_ty = list.ty(&self.cfg.locals, tcx).ty;
        let ty::Adt(adt, vec_args) = list_ty.kind() else { self.refuse("indexing a non-list value") };
        if adt.did() != vec_def(tcx) {
            self.refuse(&format!("indexing a `{list_ty}`"));
        }
        let element = vec_args.type_at(0);
        let index = self.operand(index);
        let erased = tcx.lifetimes.re_erased;
        let (mutability, borrow_kind) = if write { (Mutability::Mut, BorrowKind::Mut { kind: MutBorrowKind::Default }) } else { (Mutability::Not, BorrowKind::Shared) };
        let (as_slice, get) = if write { ("as_mut_slice", "list_get_mut") } else { ("as_slice", "list_get") };
        let list_ref = self.cfg.temp(Ty::new_ref(tcx, erased, list_ty, mutability));
        self.cfg.assign(self.current, list_ref, Rvalue::Ref(erased, borrow_kind, list), span);
        let slice = self.cfg.temp(Ty::new_ref(tcx, erased, Ty::new_slice(tcx, element), mutability));
        let as_slice = common::inherent_method(tcx, vec_def(tcx), as_slice);
        self.call(as_slice, vec_args.as_slice(), vec![Operand::Move(list_ref)], slice, span);
        let element_ref = self.cfg.temp(Ty::new_ref(tcx, erased, element, mutability));
        let get = extern_item(tcx, &["incan_std_core", "collections", get]);
        self.call(get, &[element.into()], vec![Operand::Move(slice), index], element_ref, span);
        tcx.mk_place_deref(element_ref)
    }

    /// `base.name`, by the field's position in the model's Rust struct, which keeps the declared field order.
    fn field_place(&self, base: Place<'tcx>, name: &str) -> Place<'tcx> {
        // A method's receiver is a reference to the model; its fields are read through it.
        let mut base = base;
        while let ty::Ref(..) = base.ty(&self.cfg.locals, self.tcx).ty.kind() {
            base = self.tcx.mk_place_deref(base);
        }
        let ty::Adt(adt, args) = base.ty(&self.cfg.locals, self.tcx).ty.kind() else { self.refuse("a field of a non-model value") };
        let Some((index, field)) = adt.non_enum_variant().fields.iter_enumerated().find(|(_, f)| f.name.as_str() == name) else {
            self.refuse(&format!("the field `{name}`"));
        };
        // Incan field types name no associated types, so there is nothing to normalize.
        let field_ty = field.ty(self.tcx, args).skip_normalization();
        self.tcx.mk_place_field(base, index, field_ty)
    }

    /// A shared or mutable borrow of `place`, in a fresh temporary. A place that already holds a reference, such as a
    /// method's receiver, is reborrowed rather than borrowed twice.
    fn borrow(&mut self, place: &bir::Place, mutability: Mutability, span: Span) -> Place<'tcx> {
        let tcx = self.tcx;
        let mut target = self.place_for(place, mutability == Mutability::Mut);
        if let ty::Ref(..) = target.ty(&self.cfg.locals, tcx).ty.kind() {
            target = tcx.mk_place_deref(target);
        }
        let target_ty = target.ty(&self.cfg.locals, tcx).ty;
        let erased = tcx.lifetimes.re_erased;
        let (reference_ty, kind) = match mutability {
            Mutability::Not => (Ty::new_imm_ref(tcx, erased, target_ty), BorrowKind::Shared),
            Mutability::Mut => (Ty::new_mut_ref(tcx, erased, target_ty), BorrowKind::Mut { kind: MutBorrowKind::Default }),
        };
        let reference = self.cfg.temp(reference_ty);
        self.cfg.assign(self.current, reference, Rvalue::Ref(erased, kind, target), span);
        reference
    }

    /// `Clone::clone(&place)`: Body IR's copy of a non-`Copy` value, such as a list passed where it is read again later.
    fn clone_of(&mut self, place: &bir::Place, span: Span) -> Place<'tcx> {
        let tcx = self.tcx;
        let source = self.place(place);
        let source_ty = source.ty(&self.cfg.locals, tcx).ty;
        let Some(clone_trait) = tcx.lang_items().clone_trait() else { self.refuse("copies without the `Clone` lang item") };
        let Some(clone) = tcx.associated_item_def_ids(clone_trait).iter().copied().find(|d| tcx.item_name(*d).as_str() == "clone") else {
            self.refuse("copies without `Clone::clone`");
        };
        let erased = tcx.lifetimes.re_erased;
        let borrowed = self.cfg.temp(Ty::new_imm_ref(tcx, erased, source_ty));
        self.cfg.assign(self.current, borrowed, Rvalue::Ref(erased, BorrowKind::Shared, source), span);
        let copy = self.cfg.temp(source_ty);
        self.call(clone, &[source_ty.into()], vec![Operand::Move(borrowed)], copy, span);
        copy
    }

    /// An operand where a value of `expected` type is wanted. Body IR keeps an `int` literal the checker accepted as a
    /// `float` (`f = 1`, `half(5)`) as an integer constant, so it is lowered as the float the checker meant.
    fn operand_expecting(&mut self, operand: &bir::Operand, expected: Ty<'tcx>) -> Operand<'tcx> {
        if let bir::Operand::Constant(bir::Constant::Int(value)) = operand
            && expected.is_floating_point()
        {
            let double = <rustc_apfloat::ieee::Double as rustc_apfloat::Float>::from_bits(u128::from((*value as f64).to_bits()));
            return Operand::const_from_scalar(self.tcx, self.tcx.types.f64, Scalar::from_f64(double), self.cfg.span);
        }
        self.operand(operand)
    }

    fn operand(&mut self, operand: &bir::Operand) -> Operand<'tcx> {
        let span = self.cfg.span;
        match operand {
            bir::Operand::Constant(bir::Constant::Str(text)) => {
                // A `str` literal is an owned `String`: `ToString::to_string("...")`, as the emitted route's
                // `"...".to_string()`.
                let literal = common::str_literal(self.tcx, text, span);
                let owned = self.to_string_of(literal, self.tcx.types.str_, span);
                Operand::Move(owned)
            }
            bir::Operand::Constant(bir::Constant::Int(v)) => Operand::const_from_scalar(self.tcx, self.tcx.types.i64, Scalar::from_i64(*v), span),
            bir::Operand::Constant(bir::Constant::Bool(b)) => Operand::const_from_scalar(self.tcx, self.tcx.types.bool, Scalar::from_bool(*b), span),
            bir::Operand::Constant(bir::Constant::Float(text)) => {
                let Ok(value) = text.parse::<f64>() else { self.refuse(&format!("the float literal `{text}`")) };
                let double = <rustc_apfloat::ieee::Double as rustc_apfloat::Float>::from_bits(u128::from(value.to_bits()));
                Operand::const_from_scalar(self.tcx, self.tcx.types.f64, Scalar::from_f64(double), span)
            }
            bir::Operand::Place(source) => match source.fact {
                bir::OwnershipFact::Copy => Operand::Copy(self.place(&source.place)),
                bir::OwnershipFact::Move => Operand::Move(self.place(&source.place)),
                bir::OwnershipFact::Borrow => Operand::Move(self.borrow(&source.place, Mutability::Not, span)),
                bir::OwnershipFact::MutBorrow => Operand::Move(self.borrow(&source.place, Mutability::Mut, span)),
                bir::OwnershipFact::Clone => Operand::Move(self.clone_of(&source.place, span)),
                other => self.refuse(&format!("a `{other:?}` operand")),
            },
            bir::Operand::Constant(other) => self.refuse(&format!("the constant `{other:?}`")),
        }
    }

    /// Lower `block`'s statements into the current block, opening new blocks as control flow requires.
    fn lower_block(&mut self, block: &bir::Block) {
        for stmt in &block.stmts {
            self.lower_statement(stmt);
        }
    }

    fn lower_statement(&mut self, stmt: &bir::Statement) {
        let span = self.span(&stmt.span);
        match &stmt.kind {
            bir::StatementKind::Assign { place, rvalue } => self.lower_assign(place, rvalue, span),
            bir::StatementKind::Call { destination, callee, args, .. } => self.lower_call(destination.as_ref(), callee, args, span),
            bir::StatementKind::If { cond, then_block, else_block } => self.lower_if(cond, then_block, else_block.as_ref(), span),
            bir::StatementKind::Loop { body } => self.lower_loop(body, span),
            bir::StatementKind::Break { value: None } => self.jump_out(|(_, exit)| exit, "break", span),
            bir::StatementKind::Continue => self.jump_out(|(header, _)| header, "continue", span),
            bir::StatementKind::Return { value } => self.lower_return(value.as_ref(), span),
            bir::StatementKind::IterNext { destination, iterator, .. } => self.lower_iter_next(destination, iterator, span),
            // An expression statement only discards an already-computed value.
            bir::StatementKind::Expr { .. } => {}
            bir::StatementKind::Drop { local } => {
                let place = self.places.get(&local.0).copied().unwrap_or_else(|| self.refuse("a drop of an unplanned local"));
                let next = self.cfg.block();
                self.cfg.terminate(self.current, TerminatorKind::Drop { place, target: next, unwind: UnwindAction::Continue, replace: false, drop: None }, span);
                self.current = next;
            }
            other => self.refuse(&format!("the statement `{}`", statement_name(other))),
        }
    }

    fn lower_assign(&mut self, place: &bir::Place, rvalue: &bir::Rvalue, span: Span) {
        let dest = self.place_to_write(place);
        match rvalue {
            bir::Rvalue::Use(bir::Operand::Place(source)) if source.fact == bir::OwnershipFact::Borrow => {
                if local_of(place).is_some_and(|iterator| self.list_iters.contains(&iterator)) {
                    self.lower_list_iterator(&source.place, dest, span);
                    return;
                }
                // A borrow of a range aliases it (see `plan_ranges`); nothing to emit.
                if self.place(&source.place) != dest {
                    self.refuse("borrows other than of a range");
                }
            }
            bir::Rvalue::Use(operand) => {
                let value = self.operand_expecting(operand, dest.ty(&self.cfg.locals, self.tcx).ty);
                self.cfg.assign(self.current, dest, Rvalue::Use(value, WithRetag::Yes), span);
            }
            bir::Rvalue::UnaryOp(bir::UnOp::Not, operand) => {
                let value = self.operand(operand);
                self.cfg.assign(self.current, dest, Rvalue::UnaryOp(UnOp::Not, value), span);
            }
            bir::Rvalue::BinaryOp(op, lhs, rhs) => self.lower_binary(*op, lhs, rhs, dest, span),
            bir::Rvalue::Format(parts) => self.lower_format(parts, dest, span),
            bir::Rvalue::Aggregate(bir::AggregateKind::Constructor(target), args) => self.lower_construction(target, args, dest, span),
            bir::Rvalue::Aggregate(bir::AggregateKind::List, elements) => self.lower_list(elements, dest, span),
            other => self.refuse(&format!("the rvalue `{}`", rvalue_name(other))),
        }
    }

    fn lower_binary(&mut self, op: bir::BinOp, lhs: &bir::Operand, rhs: &bir::Operand, dest: Place<'tcx>, span: Span) {
        // An `int` literal beside a `float` operand is the float the checker accepted it as.
        let (a, b) = if matches!(lhs, bir::Operand::Constant(bir::Constant::Int(_))) {
            let b = self.operand(rhs);
            let a = self.operand_expecting(lhs, b.ty(&self.cfg.locals, self.tcx));
            (a, b)
        } else {
            let a = self.operand(lhs);
            let b = self.operand_expecting(rhs, a.ty(&self.cfg.locals, self.tcx));
            (a, b)
        };
        // MIR's comparison operators take scalars only. `==` and `!=` on anything else, such as two lists, are
        // `PartialEq::eq(&a, &b)`, as Rust lowers them.
        let a_ty = a.ty(&self.cfg.locals, self.tcx);
        if !a_ty.is_scalar() {
            if !matches!(op, bir::BinOp::Eq | bir::BinOp::Ne) {
                self.refuse(&format!("the operator `{op:?}` on a `{a_ty}`"));
            }
            self.lower_partial_eq(a, b, op == bir::BinOp::Ne, dest, span);
            return;
        }
        let compare = |op| Rvalue::BinaryOp(op, Box::new((a.clone(), b.clone())));
        // `float` arithmetic is IEEE arithmetic with no overflow check, as in Rust; `int` arithmetic is checked.
        if a.ty(&self.cfg.locals, self.tcx).is_floating_point() {
            let arithmetic = match op {
                bir::BinOp::Add => Some(BinOp::Add),
                bir::BinOp::Sub => Some(BinOp::Sub),
                bir::BinOp::Mul => Some(BinOp::Mul),
                bir::BinOp::Div => Some(BinOp::Div),
                _ => None,
            };
            if let Some(arithmetic) = arithmetic {
                self.cfg.assign(self.current, dest, Rvalue::BinaryOp(arithmetic, Box::new((a, b))), span);
                return;
            }
        }
        match op {
            bir::BinOp::Add => self.checked_arithmetic(BinOp::Add, a, b, dest, span),
            bir::BinOp::Sub => self.checked_arithmetic(BinOp::Sub, a, b, dest, span),
            bir::BinOp::Mul => self.checked_arithmetic(BinOp::Mul, a, b, dest, span),
            bir::BinOp::Mod => self.runtime_helper("py_mod_i64", a, b, dest, span),
            bir::BinOp::FloorDiv => self.runtime_helper("py_floor_div_i64", a, b, dest, span),
            bir::BinOp::Eq => self.cfg.assign(self.current, dest, compare(BinOp::Eq), span),
            bir::BinOp::Ne => self.cfg.assign(self.current, dest, compare(BinOp::Ne), span),
            bir::BinOp::Lt => self.cfg.assign(self.current, dest, compare(BinOp::Lt), span),
            bir::BinOp::Le => self.cfg.assign(self.current, dest, compare(BinOp::Le), span),
            bir::BinOp::Gt => self.cfg.assign(self.current, dest, compare(BinOp::Gt), span),
            bir::BinOp::Ge => self.cfg.assign(self.current, dest, compare(BinOp::Ge), span),
            other => self.refuse(&format!("the operator `{other:?}`")),
        }
    }

    /// `PartialEq::eq(&a, &b)` into `dest`, negated for `!=`. Both operands are held in temporaries, then dropped.
    fn lower_partial_eq(&mut self, a: Operand<'tcx>, b: Operand<'tcx>, negate: bool, dest: Place<'tcx>, span: Span) {
        let tcx = self.tcx;
        let Some(eq_trait) = tcx.lang_items().eq_trait() else { self.refuse("`==` without the `PartialEq` lang item") };
        let Some(eq) = tcx.associated_item_def_ids(eq_trait).iter().copied().find(|d| tcx.item_name(*d).as_str() == "eq") else {
            self.refuse("`==` without `PartialEq::eq`");
        };
        let erased = tcx.lifetimes.re_erased;
        let mut held = Vec::new();
        let mut references = Vec::new();
        for value in [a, b] {
            let value_ty = value.ty(&self.cfg.locals, tcx);
            let place = self.cfg.temp(value_ty);
            self.cfg.assign(self.current, place, Rvalue::Use(value, WithRetag::Yes), span);
            let reference = self.cfg.temp(Ty::new_imm_ref(tcx, erased, value_ty));
            self.cfg.assign(self.current, reference, Rvalue::Ref(erased, BorrowKind::Shared, place), span);
            held.push((place, value_ty));
            references.push(Operand::Move(reference));
        }
        let generic_args = [held[0].1.into(), held[1].1.into()];
        let equal = self.cfg.temp(tcx.types.bool);
        self.call(eq, &generic_args, references, equal, span);
        let result = if negate { Rvalue::UnaryOp(UnOp::Not, Operand::Move(equal)) } else { Rvalue::Use(Operand::Move(equal), WithRetag::Yes) };
        self.cfg.assign(self.current, dest, result, span);
        for (place, _) in held {
            let next = self.cfg.block();
            self.cfg.terminate(self.current, TerminatorKind::Drop { place, target: next, unwind: UnwindAction::Continue, replace: false, drop: None }, span);
            self.current = next;
        }
    }

    /// `a <op> b` on `int`, with the same overflow check rustc's own MIR building inserts when overflow checks are on.
    fn checked_arithmetic(&mut self, op: BinOp, a: Operand<'tcx>, b: Operand<'tcx>, dest: Place<'tcx>, span: Span) {
        let tcx = self.tcx;
        if !tcx.sess.overflow_checks() {
            self.cfg.assign(self.current, dest, Rvalue::BinaryOp(op, Box::new((a, b))), span);
            return;
        }
        let with_overflow = match op {
            BinOp::Add => BinOp::AddWithOverflow,
            BinOp::Sub => BinOp::SubWithOverflow,
            _ => BinOp::MulWithOverflow,
        };
        let checked = self.cfg.temp(Ty::new_tup(tcx, &[tcx.types.i64, tcx.types.bool]));
        self.cfg.assign(self.current, checked, Rvalue::BinaryOp(with_overflow, Box::new((a.clone(), b.clone()))), span);
        let next = self.cfg.block();
        let overflowed = Operand::Move(tcx.mk_place_field(checked, FieldIdx::from_u32(1), tcx.types.bool));
        let msg = Box::new(AssertKind::Overflow(op, a, b));
        self.cfg.terminate(self.current, TerminatorKind::Assert { cond: overflowed, expected: false, msg, target: next, unwind: UnwindAction::Continue }, span);
        self.current = next;
        let value = Operand::Move(tcx.mk_place_field(checked, FieldIdx::from_u32(0), tcx.types.i64));
        self.cfg.assign(self.current, dest, Rvalue::Use(value, WithRetag::Yes), span);
    }

    /// Python-semantics `//` and `%`: a call to the stdlib runtime helper the emitted-Rust route also calls.
    fn runtime_helper(&mut self, helper: &str, a: Operand<'tcx>, b: Operand<'tcx>, dest: Place<'tcx>, span: Span) {
        let callee = extern_item(self.tcx, &["incan_std_core", "num", helper]);
        self.call(callee, &[], vec![a, b], dest, span);
    }

    fn call(&mut self, callee: rustc_hir::def_id::DefId, generic_args: &[ty::GenericArg<'tcx>], args: Vec<Operand<'tcx>>, dest: Place<'tcx>, span: Span) {
        let next = self.cfg.block();
        let kind = TerminatorKind::Call {
            func: Operand::function_handle(self.tcx, callee, generic_args.iter().copied(), span),
            args: args.into_iter().map(|node| rustc_span::Spanned { node, span }).collect(),
            destination: dest,
            target: Some(next),
            unwind: UnwindAction::Continue,
            call_source: CallSource::Normal,
            fn_span: span,
        };
        self.cfg.terminate(self.current, kind, span);
        self.current = next;
    }

    fn lower_call(&mut self, destination: Option<&bir::Place>, callee: &bir::Callee, args: &[bir::ArgumentElement], span: Span) {
        if let bir::Callee::Method(target) = callee
            && let Some(bir::ArgumentElement::One(bir::Operand::Place(receiver))) = args.first()
            && self.is_list_place(&receiver.place)
        {
            return self.lower_list_method(&target.name, &receiver.place, &args[1..], destination, span);
        }
        // Arguments to one of this unit's free functions take that function's parameter types, so an `int` literal
        // passed to a `float` parameter is lowered as a float.
        let parameter_tys: Vec<Ty<'tcx>> = match callee {
            bir::Callee::Function(bir::CallableTarget::Named(target))
                if target.builtin.is_none()
                    && MODULE.get().is_some_and(|module| module.bodies.iter().any(|body| body.name == target.name && receiver_owner(body).is_none())) =>
            {
                let sig = self.tcx.fn_sig(local_item(self.tcx, &target.name)).instantiate_identity().skip_normalization().skip_binder();
                sig.inputs().to_vec()
            }
            _ => Vec::new(),
        };
        let mut operands: Vec<Operand<'tcx>> = Vec::with_capacity(args.len());
        for (index, arg) in args.iter().enumerate() {
            let bir::ArgumentElement::One(operand) = arg else { self.refuse("named or spread arguments") };
            let lowered = match parameter_tys.get(index) {
                Some(expected) => self.operand_expecting(operand, *expected),
                None => self.operand(operand),
            };
            operands.push(lowered);
        }
        let Some(dest) = destination.map(|d| self.place(d)) else { self.refuse("calls without a destination") };
        if let bir::Callee::Method(target) = callee {
            // `receiver.m(args)` is `Model::m(receiver, args)`; the model is the receiver's type behind its borrow.
            let Some(receiver) = operands.first() else { self.refuse("a method call without a receiver") };
            let mut owner_ty = receiver.ty(&self.cfg.locals, self.tcx);
            while let ty::Ref(_, inner, _) = owner_ty.kind() {
                owner_ty = *inner;
            }
            let ty::Adt(adt, _) = owner_ty.kind() else { self.refuse(&format!("the method `.{}()` on a non-model value", target.name)) };
            if !adt.did().is_local() {
                self.refuse(&format!("the method `.{}()` on a library type", target.name));
            }
            let callee = common::inherent_method(self.tcx, adt.did(), &target.name);
            self.call(callee, &[], operands, dest, span);
            return;
        }
        let bir::Callee::Function(bir::CallableTarget::Named(target)) = callee else { self.refuse("calls other than to named functions") };
        if target.builtin == Some(BuiltinFnId::Range) {
            // `range(a, b)` is `core::ops::Range { start: a, end: b }`.
            let Some(range) = self.tcx.lang_items().range_struct() else { self.refuse("ranges without the `Range` lang item") };
            let zero = || Operand::const_from_scalar(self.tcx, self.tcx.types.i64, Scalar::from_i64(0), span);
            let (start, end) = match <[Operand<'tcx>; 2]>::try_from(operands) {
                Ok([start, end]) => (start, end),
                Err(operands) => match <[Operand<'tcx>; 1]>::try_from(operands) {
                    Ok([end]) => (zero(), end),
                    Err(_) => self.refuse("`range` with other than one or two arguments"),
                },
            };
            let kind = AggregateKind::Adt(range, VariantIdx::from_u32(0), self.tcx.mk_args(&[self.tcx.types.i64.into()]), None, None);
            self.cfg.assign(self.current, dest, Rvalue::Aggregate(Box::new(kind), IndexVec::from_raw(vec![start, end])), span);
            return;
        }
        if target.builtin == Some(BuiltinFnId::Float) {
            // `float(x)` on an `int` is Rust's `x as f64`.
            let Ok([value]) = <[Operand<'tcx>; 1]>::try_from(operands) else { self.refuse("`float` with other than one argument") };
            if !value.ty(&self.cfg.locals, self.tcx).is_integral() {
                self.refuse("`float` of a non-`int` value");
            }
            self.cfg.assign(self.current, dest, Rvalue::Cast(CastKind::IntToFloat, value, self.tcx.types.f64), span);
            return;
        }
        if target.builtin == Some(BuiltinFnId::Print) {
            // The emitter expands `println` into Rust's `println!` macro, which nothing can call. The native route calls
            // a runtime function instead; here the spike's runtime shim stands in for the stdlib's.
            let Ok([text]) = <[Operand<'tcx>; 1]>::try_from(operands) else { self.refuse("`println` with other than one argument") };
            // `println(x)` displays a non-`str` value first, as the emitted `println!("{}", x)` does.
            let text = if text.ty(&self.cfg.locals, self.tcx) == string_ty(self.tcx) { text } else { Operand::Move(self.string_of_value(text, span)) };
            self.call(extern_item(self.tcx, &["incan_native_rt", "println"]), &[], vec![text], dest, span);
            return;
        }
        if target.builtin == Some(BuiltinFnId::Len) {
            // `len(list)` is `list.len() as i64`. Body IR hands `len` its own copy of the list, which is dropped after.
            let tcx = self.tcx;
            let Ok([value]) = <[Operand<'tcx>; 1]>::try_from(operands) else { self.refuse("`len` with other than one argument") };
            let value_ty = value.ty(&self.cfg.locals, tcx);
            let ty::Adt(adt, vec_args) = value_ty.kind() else { self.refuse(&format!("`len` of a `{value_ty}`")) };
            if adt.did() != vec_def(tcx) {
                self.refuse(&format!("`len` of a `{value_ty}`"));
            }
            let held = self.cfg.temp(value_ty);
            self.cfg.assign(self.current, held, Rvalue::Use(value, WithRetag::Yes), span);
            let erased = tcx.lifetimes.re_erased;
            let borrowed = self.cfg.temp(Ty::new_imm_ref(tcx, erased, value_ty));
            self.cfg.assign(self.current, borrowed, Rvalue::Ref(erased, BorrowKind::Shared, held), span);
            let length = self.cfg.temp(tcx.types.usize);
            self.call(common::inherent_method(tcx, vec_def(tcx), "len"), vec_args.as_slice(), vec![Operand::Move(borrowed)], length, span);
            self.cfg.assign(self.current, dest, Rvalue::Cast(CastKind::IntToInt, Operand::Move(length), tcx.types.i64), span);
            let next = self.cfg.block();
            self.cfg.terminate(self.current, TerminatorKind::Drop { place: held, target: next, unwind: UnwindAction::Continue, replace: false, drop: None }, span);
            self.current = next;
            return;
        }
        if target.builtin.is_some() {
            self.refuse(&format!("the builtin `{}`", target.name));
        }
        if !MODULE.get().is_some_and(|module| module.bodies.iter().any(|body| body.name == target.name)) {
            self.refuse(&format!("calls to `{}`, which this unit does not define (an import or an alias)", target.name));
        }
        self.call(local_item(self.tcx, &target.name), &[], operands, dest, span);
    }

    /// An f-string: `incan_std_core::strings::fstring(parts, args)`, the runtime call the emitted route makes. `parts`
    /// holds one more literal than there are values, empty where two values or an end meet; each value becomes a
    /// `String` through `ToString::to_string`, which is `Display` for `int` and `str`.
    /// `Model(field=value, ..)`: Body IR binds the arguments to the model's declared field order, which is the order of
    /// the Rust struct the driver declared, so they become its fields positionally.
    fn lower_construction(&mut self, target: &bir::ConstructorTarget, args: &[bir::ArgumentElement], dest: Place<'tcx>, span: Span) {
        let model = local_item(self.tcx, &target.name);
        let field_tys: Vec<Ty<'tcx>> = self.tcx.adt_def(model).non_enum_variant().fields.iter().map(|f| self.tcx.type_of(f.did).instantiate_identity().skip_normalization()).collect();
        let mut fields = Vec::with_capacity(args.len());
        for (arg, field_ty) in args.iter().zip(field_tys) {
            let bir::ArgumentElement::One(operand) = arg else { self.refuse("named or spread constructor arguments") };
            fields.push(self.operand_expecting(operand, field_ty));
        }
        let kind = AggregateKind::Adt(model, VariantIdx::from_u32(0), self.tcx.mk_args(&[]), None, None);
        self.cfg.assign(self.current, dest, Rvalue::Aggregate(Box::new(kind), IndexVec::from_raw(fields)), span);
    }

    /// Whether `place` is a whole `List` local, as Body IR types it.
    fn is_list_place(&self, place: &bir::Place) -> bool {
        let Some(local) = local_of(place) else { return false };
        self.body.locals.iter().any(|l| l.id.0 == local && matches!(&l.ty, IncanType::Generic { base, .. } if base == "List"))
    }

    /// A method on a list. Body IR passes the receiver of a mutating method such as `append` as a shared borrow, so
    /// the receiver is borrowed mutably here, as the emitted route's `list.push(x)` does.
    fn lower_list_method(&mut self, name: &str, receiver: &bir::Place, args: &[bir::ArgumentElement], destination: Option<&bir::Place>, span: Span) {
        let tcx = self.tcx;
        let rust_name = match name {
            "append" => "push",
            other => self.refuse(&format!("the list method `.{other}()`")),
        };
        let mut list = self.place_to_write(receiver);
        while let ty::Ref(..) = list.ty(&self.cfg.locals, tcx).ty.kind() {
            list = tcx.mk_place_deref(list);
        }
        let list_ty = list.ty(&self.cfg.locals, tcx).ty;
        let ty::Adt(_, vec_args) = list_ty.kind() else { self.refuse("a list method on a non-list value") };
        let erased = tcx.lifetimes.re_erased;
        let list_ref = self.cfg.temp(Ty::new_mut_ref(tcx, erased, list_ty));
        self.cfg.assign(self.current, list_ref, Rvalue::Ref(erased, BorrowKind::Mut { kind: MutBorrowKind::Default }, list), span);
        let mut operands = vec![Operand::Move(list_ref)];
        for arg in args {
            let bir::ArgumentElement::One(value) = arg else { self.refuse("named or spread arguments") };
            operands.push(self.operand(value));
        }
        let Some(dest) = destination.map(|d| self.place_to_write(d)) else { self.refuse("calls without a destination") };
        self.call(common::inherent_method(tcx, vec_def(tcx), rust_name), vec_args.as_slice(), operands, dest, span);
    }

    /// `[a, b, ..]`: `Vec::new()`, then `push` for each element in order, which is what `vec![a, b, ..]` does.
    fn lower_list(&mut self, elements: &[bir::ArgumentElement], dest: Place<'tcx>, span: Span) {
        let tcx = self.tcx;
        let list_ty = dest.ty(&self.cfg.locals, tcx).ty;
        let ty::Adt(_, vec_args) = list_ty.kind() else { self.refuse("a list literal of a non-list type") };
        let element = vec_args.type_at(0);
        self.call(common::inherent_method(tcx, vec_def(tcx), "new"), &[element.into()], vec![], dest, span);
        let push = common::inherent_method(tcx, vec_def(tcx), "push");
        for element_arg in elements {
            let bir::ArgumentElement::One(value) = element_arg else { self.refuse("a spread in a list literal") };
            let value = self.operand_expecting(value, element);
            let erased = tcx.lifetimes.re_erased;
            let list = self.cfg.temp(Ty::new_mut_ref(tcx, erased, list_ty));
            self.cfg.assign(self.current, list, Rvalue::Ref(erased, BorrowKind::Mut { kind: MutBorrowKind::Default }, dest), span);
            let unit = self.cfg.temp(tcx.types.unit);
            self.call(push, vec_args.as_slice(), vec![Operand::Move(list), value], unit, span);
        }
    }

    fn lower_format(&mut self, format: &[bir::FormatPart], dest: Place<'tcx>, span: Span) {
        let tcx = self.tcx;
        let (mut literals, mut values, mut pending) = (Vec::new(), Vec::new(), String::new());
        for part in format {
            match part {
                bir::FormatPart::Literal(text) => pending.push_str(text),
                bir::FormatPart::Expr { operand, style: bir::FormatStyle::Display } => {
                    literals.push(std::mem::take(&mut pending));
                    values.push(self.display_string(operand, span));
                }
                bir::FormatPart::Expr { .. } => self.refuse("`{x!r}`-style debug formatting"),
            }
        }
        literals.push(pending);

        let erased = tcx.lifetimes.re_erased;
        let str_ref = Ty::new_imm_ref(tcx, erased, tcx.types.str_);
        let parts: Vec<Operand<'tcx>> = literals.iter().map(|text| common::str_literal(tcx, text, span)).collect();
        let parts = self.slice_of(str_ref, parts, span);
        let string = string_ty(tcx);
        let args_array = self.cfg.temp(Ty::new_array(tcx, string, values.len() as u64));
        let moved: Vec<Operand<'tcx>> = values.into_iter().map(Operand::Move).collect();
        self.cfg.assign(self.current, args_array, Rvalue::Aggregate(Box::new(AggregateKind::Array(string)), IndexVec::from_raw(moved)), span);
        let args = self.borrow_as_slice(args_array, string, span);
        self.call(extern_item(tcx, &["incan_std_core", "strings", "fstring"]), &[], vec![Operand::Move(parts), Operand::Move(args)], dest, span);
        let next = self.cfg.block();
        self.cfg.terminate(self.current, TerminatorKind::Drop { place: args_array, target: next, unwind: UnwindAction::Continue, replace: false, drop: None }, span);
        self.current = next;
    }

    /// The text of a displayed f-string or `println` value, as a fresh `String`. A borrowed place, such as
    /// `borrow(user.name)`, is displayed where it is, without a copy.
    fn display_string(&mut self, value: &bir::Operand, span: Span) -> Place<'tcx> {
        if let bir::Operand::Place(source) = value
            && source.fact == bir::OwnershipFact::Borrow
        {
            let place = self.place(&source.place);
            return self.display_place(place, span);
        }
        let value = self.operand(value);
        self.string_of_value(value, span)
    }

    /// Display an already-lowered value: hold it in a temporary, then display that place.
    fn string_of_value(&mut self, value: Operand<'tcx>, span: Span) -> Place<'tcx> {
        let held = self.cfg.temp(value.ty(&self.cfg.locals, self.tcx));
        self.cfg.assign(self.current, held, Rvalue::Use(value, WithRetag::Yes), span);
        self.display_place(held, span)
    }

    /// Display the value in `place`, behind any references. `float` is spelled the way the stdlib spells it for the
    /// emitted route, `incan_std_core::strings::float_to_string`, so `100.0` stays visibly a float; `int`, `bool` and
    /// `str` display through `ToString`, which is `Display`.
    fn display_place(&mut self, place: Place<'tcx>, span: Span) -> Place<'tcx> {
        let tcx = self.tcx;
        let mut place = place;
        while let ty::Ref(..) = place.ty(&self.cfg.locals, tcx).ty.kind() {
            place = tcx.mk_place_deref(place);
        }
        let place_ty = place.ty(&self.cfg.locals, tcx).ty;
        if place_ty.is_floating_point() {
            let text = self.cfg.temp(string_ty(tcx));
            let float_to_string = extern_item(tcx, &["incan_std_core", "strings", "float_to_string"]);
            self.call(float_to_string, &[place_ty.into()], vec![Operand::Copy(place)], text, span);
            return text;
        }
        if !(place_ty.is_integral() || place_ty.is_bool() || place_ty == string_ty(tcx) || place_ty.is_str()) {
            self.refuse(&format!("displaying a value of type `{place_ty}`"));
        }
        let erased = tcx.lifetimes.re_erased;
        let borrowed = self.cfg.temp(Ty::new_imm_ref(tcx, erased, place_ty));
        self.cfg.assign(self.current, borrowed, Rvalue::Ref(erased, BorrowKind::Shared, place), span);
        self.to_string_of(Operand::Move(borrowed), place_ty, span)
    }

    /// `ToString::to_string(reference)` for a reference to a `self_ty`, into a fresh `String`.
    fn to_string_of(&mut self, reference: Operand<'tcx>, self_ty: Ty<'tcx>, span: Span) -> Place<'tcx> {
        let tcx = self.tcx;
        let Some(to_string) = tcx.get_diagnostic_item(rustc_span::Symbol::intern("to_string_method")) else { self.refuse("displaying without `ToString`") };
        let text = self.cfg.temp(string_ty(tcx));
        self.call(to_string, &[self_ty.into()], vec![reference], text, span);
        text
    }

    /// `&[elements]` as a `&[T]` slice: an array temporary, borrowed, then unsized as Rust coerces `&[T; N]`.
    fn slice_of(&mut self, element: Ty<'tcx>, elements: Vec<Operand<'tcx>>, span: Span) -> Place<'tcx> {
        let array = self.cfg.temp(Ty::new_array(self.tcx, element, elements.len() as u64));
        self.cfg.assign(self.current, array, Rvalue::Aggregate(Box::new(AggregateKind::Array(element)), IndexVec::from_raw(elements)), span);
        self.borrow_as_slice(array, element, span)
    }

    fn borrow_as_slice(&mut self, array: Place<'tcx>, element: Ty<'tcx>, span: Span) -> Place<'tcx> {
        let tcx = self.tcx;
        let erased = tcx.lifetimes.re_erased;
        let array_ty = array.ty(&self.cfg.locals, tcx).ty;
        let array_ref = self.cfg.temp(Ty::new_imm_ref(tcx, erased, array_ty));
        self.cfg.assign(self.current, array_ref, Rvalue::Ref(erased, BorrowKind::Shared, array), span);
        let slice_ref_ty = Ty::new_imm_ref(tcx, erased, Ty::new_slice(tcx, element));
        let slice = self.cfg.temp(slice_ref_ty);
        let unsize = CastKind::PointerCoercion(rustc_middle::ty::adjustment::PointerCoercion::Unsize, CoercionSource::Implicit);
        self.cfg.assign(self.current, slice, Rvalue::Cast(unsize, Operand::Move(array_ref), slice_ref_ty), span);
        slice
    }

    fn lower_if(&mut self, cond: &bir::Operand, then_block: &bir::Block, else_block: Option<&bir::Block>, span: Span) {
        let discr = self.operand(cond);
        let (then_bb, else_bb, join) = (self.cfg.block(), self.cfg.block(), self.cfg.block());
        self.cfg.terminate(self.current, TerminatorKind::SwitchInt { discr, targets: SwitchTargets::static_if(0, else_bb, then_bb) }, span);
        for (bb, block) in [(then_bb, Some(then_block)), (else_bb, else_block)] {
            self.current = bb;
            if let Some(block) = block {
                self.lower_block(block);
            }
            self.cfg.terminate(self.current, TerminatorKind::Goto { target: join }, span);
        }
        self.current = join;
    }

    fn lower_loop(&mut self, body: &bir::Block, span: Span) {
        let (header, exit) = (self.cfg.block(), self.cfg.block());
        self.cfg.terminate(self.current, TerminatorKind::Goto { target: header }, span);
        self.current = header;
        self.loops.push((header, exit));
        self.lower_block(body);
        self.loops.pop();
        self.cfg.terminate(self.current, TerminatorKind::Goto { target: header }, span);
        self.current = exit;
    }

    /// `break` or `continue`: jump to the innermost loop's target; later statements land in a fresh, unreachable block.
    fn jump_out(&mut self, target: impl Fn((BasicBlock, BasicBlock)) -> BasicBlock, what: &str, span: Span) {
        let Some(&innermost) = self.loops.last() else { self.refuse(&format!("`{what}` outside a loop")) };
        self.cfg.terminate(self.current, TerminatorKind::Goto { target: target(innermost) }, span);
        self.current = self.cfg.block();
    }

    fn lower_return(&mut self, value: Option<&bir::Operand>, span: Span) {
        let Some(value) = value else { self.refuse("`return` without a value") };
        let return_ty = Place::return_place().ty(&self.cfg.locals, self.tcx).ty;
        let value = self.operand_expecting(value, return_ty);
        self.cfg.assign(self.current, Place::return_place(), Rvalue::Use(value, WithRetag::Yes), span);
        self.cfg.terminate(self.current, TerminatorKind::Goto { target: self.exit }, span);
        self.current = self.cfg.block();
    }

    /// One poll of a `for` loop over a range: `Iterator::next(&mut range)`; `None` breaks the innermost loop and
    /// `Some(item)` stores `item`, as Rust's own `for` loop desugaring does.
    fn lower_iter_next(&mut self, destination: &bir::Place, iterator: &bir::Operand, span: Span) {
        let tcx = self.tcx;
        let bir::Operand::Place(source) = iterator else { self.refuse("polling a non-place iterator") };
        let iterator = self.place(&source.place);
        let iterator_ty = iterator.ty(&self.cfg.locals, tcx).ty;
        let erased = tcx.lifetimes.re_erased;
        // A range yields `i64`; a list's `slice::Iter<T>` yields `&T`.
        let item_ty = match iterator_ty.kind() {
            ty::Adt(adt, args) if Some(adt.did()) == tcx.lang_items().range_struct() => args.type_at(0),
            ty::Adt(adt, args) if Some(adt.did()) == tcx.get_diagnostic_item(rustc_span::Symbol::intern("SliceIter")) => {
                Ty::new_imm_ref(tcx, erased, args.type_at(1))
            }
            _ => self.refuse(&format!("polling a `{iterator_ty}`")),
        };
        let (Some(next_fn), Some(option)) = (tcx.lang_items().next_fn(), tcx.lang_items().option_type()) else { self.refuse("iteration without the `Iterator` and `Option` lang items") };
        let option_ty = Ty::new_adt(tcx, tcx.adt_def(option), tcx.mk_args(&[item_ty.into()]));
        let borrowed = self.cfg.temp(Ty::new_mut_ref(tcx, erased, iterator_ty));
        self.cfg.assign(self.current, borrowed, Rvalue::Ref(erased, BorrowKind::Mut { kind: MutBorrowKind::Default }, iterator), span);
        let polled = self.cfg.temp(option_ty);
        self.call(next_fn, &[iterator_ty.into()], vec![Operand::Move(borrowed)], polled, span);

        let discr = self.cfg.temp(option_ty.discriminant_ty(tcx));
        self.cfg.assign(self.current, discr, Rvalue::Discriminant(polled), span);
        let Some(&(_, exit)) = self.loops.last() else { self.refuse("`for` polling outside a loop") };
        let produced = self.cfg.block();
        self.cfg.terminate(self.current, TerminatorKind::SwitchInt { discr: Operand::Move(discr), targets: SwitchTargets::static_if(0, exit, produced) }, span);
        self.current = produced;
        let some = tcx.mk_place_downcast(polled, tcx.adt_def(option), VariantIdx::from_u32(1));
        let mut item = tcx.mk_place_field(some, FieldIdx::from_u32(0), item_ty);
        if item_ty.is_ref() {
            item = tcx.mk_place_deref(item);
        }
        let dest = self.place_to_write(destination);
        self.cfg.assign(self.current, dest, Rvalue::Use(Operand::Copy(item), WithRetag::Yes), span);
    }
}

fn local_of(place: &bir::Place) -> Option<u32> {
    match (&place.root, place.projection.is_empty()) {
        (bir::PlaceRoot::Local(local), true) => Some(local.0),
        _ => None,
    }
}

/// Whether `body` calls the `println` builtin, so the unit needs the runtime crate that provides it.
fn calls_println(body: &bir::Body) -> bool {
    let mut found = false;
    visit_statements(&body.block, &mut |stmt| {
        if let bir::StatementKind::Call { callee: bir::Callee::Function(bir::CallableTarget::Named(target)), .. } = &stmt.kind
            && target.builtin == Some(BuiltinFnId::Print)
        {
            found = true;
        }
    });
    found
}

fn is_builtin_range(callee: &bir::Callee) -> bool {
    matches!(callee, bir::Callee::Function(bir::CallableTarget::Named(target)) if target.builtin == Some(BuiltinFnId::Range))
}

/// Visit every statement in `block`, including those nested in `if` and `loop`.
fn visit_statements(block: &bir::Block, visit: &mut impl FnMut(&bir::Statement)) {
    for stmt in &block.stmts {
        visit(stmt);
        match &stmt.kind {
            bir::StatementKind::If { then_block, else_block, .. } => {
                visit_statements(then_block, visit);
                if let Some(block) = else_block {
                    visit_statements(block, visit);
                }
            }
            bir::StatementKind::Loop { body } => visit_statements(body, visit),
            _ => {}
        }
    }
}

fn statement_name(kind: &bir::StatementKind) -> String {
    format!("{kind:?}").split([' ', '{', '(']).next().unwrap_or("?").to_string()
}

fn rvalue_name(rvalue: &bir::Rvalue) -> String {
    format!("{rvalue:?}").split([' ', '{', '(']).next().unwrap_or("?").to_string()
}

fn lower_body<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId, body: &bir::Body) -> Body<'tcx> {
    let mut cfg = Cfg::new(tcx, def);
    let exit = cfg.block();
    cfg.terminate(exit, TerminatorKind::Return, cfg.span);
    let mut lowering = Lowering { tcx, body, cfg, places: HashMap::new(), current: BasicBlock::from_u32(0), loops: Vec::new(), exit, list_iters: std::collections::HashSet::new() };
    lowering.plan_locals();
    lowering.lower_block(&body.block);
    // A `None`-returning function returns `()` when it falls off its end. For a function that returns a value, falling
    // off the end is unreachable: the checker proved every path returns, and the block a `return` leaves behind is
    // never entered.
    if body.return_type == IncanType::Primitive(IncanPrimitiveType::Unit) {
        let unit = Rvalue::Aggregate(Box::new(AggregateKind::Tuple), IndexVec::new());
        lowering.cfg.assign(lowering.current, Place::return_place(), unit, lowering.cfg.span);
        lowering.cfg.terminate(lowering.current, TerminatorKind::Goto { target: exit }, lowering.cfg.span);
    } else {
        lowering.cfg.terminate(lowering.current, TerminatorKind::Unreachable, lowering.cfg.span);
    }
    lowering.cfg.finish()
}

/// The Body IR body rustc's item `def` stands for: a method by its `impl`'s self type and its name, a free function by
/// its name and the absence of a receiver.
fn body_for(tcx: TyCtxt<'_>, def: LocalDefId) -> Option<&'static bir::Body> {
    let module = MODULE.get()?;
    let name = tcx.opt_item_name(def.to_def_id())?.to_string();
    let owner = match tcx.def_kind(def) {
        rustc_hir::def::DefKind::AssocFn => {
            let impl_ty = tcx.type_of(tcx.parent(def.to_def_id())).instantiate_identity().skip_normalization();
            let ty::Adt(adt, _) = impl_ty.kind() else { return None };
            Some(tcx.item_name(adt.did()).to_string())
        }
        rustc_hir::def::DefKind::Fn => None,
        _ => return None,
    };
    module.bodies.iter().find(|body| body.name == name && receiver_owner(body).map(str::to_string) == owner)
}

fn mir_built<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> &'tcx Steal<Body<'tcx>> {
    match body_for(tcx, def) {
        Some(body) => tcx.alloc_steal_mir(lower_body(tcx, def, body)),
        None => (rustc_interface::DEFAULT_QUERY_PROVIDERS.queries.mir_built)(tcx, def),
    }
}

// ============================================================================
// Driver
// ============================================================================

struct Callbacks {
    source: PathBuf,
}

impl rustc_driver::Callbacks for Callbacks {
    fn config(&mut self, config: &mut rustc_interface::interface::Config) {
        config.input = rustc_session::config::Input::Str { name: FileName::Custom("kernels.incn".to_string()), input: String::new() };
        config.override_queries = Some(|_sess, providers| providers.queries.mir_built = mir_built);
    }

    fn after_crate_root_parsing(&mut self, compiler: &rustc_interface::interface::Compiler, krate: &mut ast::Crate) -> rustc_driver::Compilation {
        let dcx = compiler.sess.dcx();
        let file = compiler.sess.source_map().load_file(&self.source)
            .unwrap_or_else(|e| dcx.fatal(format!("cannot load Incan source {}: {e}", self.source.display())));
        let Some(text) = file.src.as_deref() else { dcx.fatal("the loaded Incan source has no text") };
        let (module, fieldless_models) = check_and_build(text).unwrap_or_else(|e| dcx.fatal(e));
        let _ = SOURCE_START.set(file.start_pos);
        let span = krate.spans.inner_span;
        krate.items.push(extern_crate("incan_std_core", span));
        if module.bodies.iter().any(calls_println) {
            krate.items.push(extern_crate("incan_native_rt", span));
        }
        let _ = MODELS.set(module.nominal_declarations.iter().map(|model_decl| model_decl.name.clone()).chain(fieldless_models.iter().cloned()).collect());
        for name in &fieldless_models {
            krate.items.push(public(model(name, ast::Generics::default(), &[], span)));
        }
        // Each model is a Rust struct with its fields in declared order, the order constructions bind to.
        for model_decl in &module.nominal_declarations {
            if model_decl.type_parameter_count > 0 {
                dcx.fatal(format!("native lowering does not support the generic model `{}` yet", model_decl.name));
            }
            let fields: Option<Vec<(&str, common::TySpec)>> = model_decl.fields.iter().zip(&model_decl.field_types)
                .map(|(field, ty)| type_spec(ty).map(|ty| (field.as_str(), ty)))
                .collect();
            let Some(fields) = fields else { dcx.fatal(format!("native lowering does not support a field type of `{}` yet", model_decl.name)) };
            krate.items.push(public(model(&model_decl.name, ast::Generics::default(), &fields, span)));
        }
        // Free functions become crate-root items; each model's methods become one inherent `impl` block.
        let mut methods: std::collections::BTreeMap<&str, thin_vec::ThinVec<Box<ast::AssocItem>>> = std::collections::BTreeMap::new();
        for body in &module.bodies {
            let receiver = body.locals.iter().find(|l| matches!(l.origin, bir::LocalOrigin::Receiver { .. })).map(|l| l.id);
            let ty_of = |local: &bir::LocalId| body.locals.iter().find(|l| l.id == *local).and_then(|l| type_spec(&l.ty));
            let params: Option<Vec<(&str, common::TySpec)>> = body.param_locals.iter().zip(&body.params)
                .filter(|(local, _)| Some(**local) != receiver)
                .map(|(local, param)| ty_of(local).map(|ty| (param.name.as_str(), ty)))
                .collect();
            let (Some(params), Some(ret)) = (params, type_spec(&body.return_type)) else {
                let unsupported = body.param_locals.iter()
                    .filter(|local| Some(**local) != receiver)
                    .filter_map(|local| body.locals.iter().find(|l| l.id == *local))
                    .map(|l| &l.ty)
                    .chain(std::iter::once(&body.return_type))
                    .find(|ty| type_spec(ty).is_none());
                dcx.fatal(format!("native lowering of `{}` does not support a signature type `{}` yet", body.name, unsupported.map(type_label).unwrap_or_default()));
            };
            match receiver_owner(body) {
                Some(owner) if is_unit_model(owner) => {
                    methods.entry(owner).or_default().push(common::method(&body.name, receiver_is_mutable(body), &params, ret, span));
                }
                Some(owner) => dcx.fatal(format!("native lowering of `{}` does not support methods of `{owner}`, which is not a model, yet", body.name)),
                None => krate.items.push(public(function(&body.name, ast::Generics::default(), &params, ret, span))),
            }
        }
        for (owner, items) in methods {
            krate.items.push(common::impl_block(owner, items, span));
        }
        let _ = MODULE.set(module);
        rustc_driver::Compilation::Continue
    }
}

fn main() {
    let mut source = None;
    let mut args = Vec::new();
    let mut raw = std::env::args();
    while let Some(arg) = raw.next() {
        if arg == "--incan-source" {
            source = raw.next().map(PathBuf::from);
        } else {
            args.push(arg);
        }
    }
    let Some(source) = source else {
        eprintln!("pass --incan-source <file.incn>");
        std::process::exit(2);
    };
    rustc_driver::run_compiler(&args, &mut Callbacks { source });
}

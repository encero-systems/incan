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

use common::{Cfg, extern_crate, extern_item, function, local_item, public, t};
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

// ============================================================================
// Front end: the real Incan checker and Body IR builder, in-process
// ============================================================================

fn check_and_build(source: &str) -> Result<bir::BodyIrModule, String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lexing failed: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("parsing failed: {errors:?}"))?;
    let mut checker = TypeChecker::new();
    let module_path = vec!["kernels".to_string()];
    checker.set_current_module_path(Some(module_path.clone()));
    checker.check_program(&program).map_err(|errors| format!("checking failed: {errors:?}"))?;
    Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
}

/// The rustc spelling of an Incan type this lowering supports.
fn type_name(ty: &IncanType) -> Option<&'static str> {
    match ty {
        IncanType::Primitive(IncanPrimitiveType::Int) => Some("i64"),
        IncanType::Primitive(IncanPrimitiveType::Bool) => Some("bool"),
        IncanType::Primitive(IncanPrimitiveType::Float) => Some("f64"),
        _ => None,
    }
}

fn mir_ty<'tcx>(tcx: TyCtxt<'tcx>, ty: &IncanType) -> Option<Ty<'tcx>> {
    match ty {
        IncanType::Primitive(IncanPrimitiveType::Int) => Some(tcx.types.i64),
        IncanType::Primitive(IncanPrimitiveType::Bool) => Some(tcx.types.bool),
        IncanType::Primitive(IncanPrimitiveType::Float) => Some(tcx.types.f64),
        _ => None,
    }
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

    fn place(&self, place: &bir::Place) -> Place<'tcx> {
        if !place.projection.is_empty() {
            self.refuse("place projections");
        }
        let Some(local) = local_of(place) else { self.refuse("global places") };
        self.places.get(&local).copied().unwrap_or_else(|| self.refuse("an unplanned local"))
    }

    fn operand(&self, operand: &bir::Operand) -> Operand<'tcx> {
        let span = self.cfg.span;
        match operand {
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
        let dest = self.place(place);
        match rvalue {
            bir::Rvalue::Use(bir::Operand::Place(source)) if source.fact == bir::OwnershipFact::Borrow => {
                // A borrow of a range aliases it (see `plan_ranges`); nothing to emit.
                if self.place(&source.place) != dest {
                    self.refuse("borrows other than of a range");
                }
            }
            bir::Rvalue::Use(operand) => {
                let value = self.operand(operand);
                self.cfg.assign(self.current, dest, Rvalue::Use(value, WithRetag::Yes), span);
            }
            bir::Rvalue::UnaryOp(bir::UnOp::Not, operand) => {
                let value = self.operand(operand);
                self.cfg.assign(self.current, dest, Rvalue::UnaryOp(UnOp::Not, value), span);
            }
            bir::Rvalue::BinaryOp(op, lhs, rhs) => self.lower_binary(*op, lhs, rhs, dest, span),
            other => self.refuse(&format!("the rvalue `{}`", rvalue_name(other))),
        }
    }

    fn lower_binary(&mut self, op: bir::BinOp, lhs: &bir::Operand, rhs: &bir::Operand, dest: Place<'tcx>, span: Span) {
        let (a, b) = (self.operand(lhs), self.operand(rhs));
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
        let operands: Vec<Operand<'tcx>> = args.iter().map(|arg| match arg {
            bir::ArgumentElement::One(operand) => self.operand(operand),
            _ => self.refuse("named or spread arguments"),
        }).collect();
        let Some(dest) = destination.map(|d| self.place(d)) else { self.refuse("calls without a destination") };
        let bir::Callee::Function(bir::CallableTarget::Named(target)) = callee else { self.refuse("calls other than to named functions") };
        if target.builtin == Some(BuiltinFnId::Range) {
            // `range(a, b)` is `core::ops::Range { start: a, end: b }`.
            let Some(range) = self.tcx.lang_items().range_struct() else { self.refuse("ranges without the `Range` lang item") };
            let [start, end] = <[Operand<'tcx>; 2]>::try_from(operands).unwrap_or_else(|_| self.refuse("`range` with other than two arguments"));
            let kind = AggregateKind::Adt(range, VariantIdx::from_u32(0), self.tcx.mk_args(&[self.tcx.types.i64.into()]), None, None);
            self.cfg.assign(self.current, dest, Rvalue::Aggregate(Box::new(kind), IndexVec::from_raw(vec![start, end])), span);
            return;
        }
        if target.builtin.is_some() {
            self.refuse(&format!("the builtin `{}`", target.name));
        }
        self.call(local_item(self.tcx, &target.name), &[], operands, dest, span);
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
        let value = self.operand(value);
        self.cfg.assign(self.current, Place::return_place(), Rvalue::Use(value, WithRetag::Yes), span);
        self.cfg.terminate(self.current, TerminatorKind::Goto { target: self.exit }, span);
        self.current = self.cfg.block();
    }

    /// One poll of a `for` loop over a range: `Iterator::next(&mut range)`; `None` breaks the innermost loop and
    /// `Some(item)` stores `item`, as Rust's own `for` loop desugaring does.
    fn lower_iter_next(&mut self, destination: &bir::Place, iterator: &bir::Operand, span: Span) {
        let tcx = self.tcx;
        let bir::Operand::Place(source) = iterator else { self.refuse("polling a non-place iterator") };
        let range = self.place(&source.place);
        let range_ty = self.range_ty();
        let erased = tcx.lifetimes.re_erased;
        let (Some(next_fn), Some(option)) = (tcx.lang_items().next_fn(), tcx.lang_items().option_type()) else { self.refuse("iteration without the `Iterator` and `Option` lang items") };
        let option_ty = Ty::new_adt(tcx, tcx.adt_def(option), tcx.mk_args(&[tcx.types.i64.into()]));
        let borrowed = self.cfg.temp(Ty::new_mut_ref(tcx, erased, range_ty));
        self.cfg.assign(self.current, borrowed, Rvalue::Ref(erased, BorrowKind::Mut { kind: MutBorrowKind::Default }, range), span);
        let polled = self.cfg.temp(option_ty);
        self.call(next_fn, &[range_ty.into()], vec![Operand::Move(borrowed)], polled, span);

        let discr = self.cfg.temp(option_ty.discriminant_ty(tcx));
        self.cfg.assign(self.current, discr, Rvalue::Discriminant(polled), span);
        let Some(&(_, exit)) = self.loops.last() else { self.refuse("`for` polling outside a loop") };
        let produced = self.cfg.block();
        self.cfg.terminate(self.current, TerminatorKind::SwitchInt { discr: Operand::Move(discr), targets: SwitchTargets::static_if(0, exit, produced) }, span);
        self.current = produced;
        let some = tcx.mk_place_downcast(polled, tcx.adt_def(option), VariantIdx::from_u32(1));
        let item = tcx.mk_place_field(some, FieldIdx::from_u32(0), tcx.types.i64);
        let dest = self.place(destination);
        self.cfg.assign(self.current, dest, Rvalue::Use(Operand::Copy(item), WithRetag::Yes), span);
    }
}

fn local_of(place: &bir::Place) -> Option<u32> {
    match (&place.root, place.projection.is_empty()) {
        (bir::PlaceRoot::Local(local), true) => Some(local.0),
        _ => None,
    }
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
    let mut lowering = Lowering { tcx, body, cfg, places: HashMap::new(), current: BasicBlock::from_u32(0), loops: Vec::new(), exit };
    lowering.plan_locals();
    lowering.lower_block(&body.block);
    // Falling off the end of a body is unreachable for a function that returns a value: the checker proved every
    // path returns, and the block a `return` leaves behind is never entered.
    lowering.cfg.terminate(lowering.current, TerminatorKind::Unreachable, lowering.cfg.span);
    lowering.cfg.finish()
}

fn mir_built<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> &'tcx Steal<Body<'tcx>> {
    let name = tcx.opt_item_name(def.to_def_id()).map(|n| n.to_string());
    let body = MODULE.get().and_then(|module| module.bodies.iter().find(|b| Some(&b.name) == name.as_ref()));
    match body {
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
        let module = check_and_build(text).unwrap_or_else(|e| dcx.fatal(e));
        let _ = SOURCE_START.set(file.start_pos);
        let span = krate.spans.inner_span;
        krate.items.push(extern_crate("incan_std_core", span));
        for body in &module.bodies {
            let ty_of = |local: &bir::LocalId| body.locals.iter().find(|l| l.id == *local).and_then(|l| type_name(&l.ty));
            let params: Option<Vec<(&str, common::TySpec)>> = body.param_locals.iter().zip(&body.params)
                .map(|(local, param)| ty_of(local).map(|ty| (param.name.as_str(), t(ty))))
                .collect();
            let (Some(params), Some(ret)) = (params, type_name(&body.return_type)) else {
                dcx.fatal(format!("native lowering does not support the signature of `{}` yet", body.name));
            };
            krate.items.push(public(function(&body.name, ast::Generics::default(), &params, t(ret), span)));
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

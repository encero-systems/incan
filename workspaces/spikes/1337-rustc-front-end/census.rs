//! Native-lowering census: how much of a corpus the step 11/12 lowering covers, and what blocks the rest.
//!
//! For every `.incn` file given, this runs the real front end (lex, parse, check, `build_body_ir_module_v0`) and walks
//! each body, recording every construct outside the subset `step11_body_ir_lowering.rs` lowers today. A body is
//! covered when it records none. The report ranks missing constructs by how many bodies each one blocks, which is the
//! order the lowering should grow in. Files that do not check on their own (most need a project around them) are
//! counted separately rather than guessed at.
//!
//! The subset here mirrors the lowering by hand; it is a planning instrument, not a second implementation.

use incan_frontend::body_ir::build_body_ir_module_v0;
use incan_frontend::{lexer, parser, typechecker::TypeChecker};
use incan_lang::lang::builtins::BuiltinFnId;
use incan_semantics_core::body_ir as bir;
use incan_semantics_core::{IncanPrimitiveType, IncanType};
use std::collections::{BTreeMap, BTreeSet};

fn type_key(ty: &IncanType) -> Option<String> {
    match ty {
        IncanType::Primitive(IncanPrimitiveType::Int | IncanPrimitiveType::Bool | IncanPrimitiveType::Float | IncanPrimitiveType::Unit | IncanPrimitiveType::Str) => None,
        IncanType::Primitive(other) => Some(format!("type {other:?}")),
        IncanType::Generic { base, .. } => Some(format!("type {base}[..]")),
        IncanType::Named(_) => Some("type nominal (model/class/enum/newtype)".to_string()),
        IncanType::Tuple(_) => Some("type tuple".to_string()),
        IncanType::Function { .. } => Some("type function value".to_string()),
        IncanType::RustInteropPath(_) => Some("type Rust interop".to_string()),
        IncanType::TypeVar(_) => Some("type generic parameter".to_string()),
        other => Some(format!("type {}", format!("{other:?}").split(['(', ' ', '{']).next().unwrap_or("?"))),
    }
}

struct Census<'a> {
    missing: BTreeSet<String>,
    ranges: BTreeSet<u32>,
    body: &'a bir::Body,
}

impl Census<'_> {
    fn local_of(place: &bir::Place) -> Option<u32> {
        match (&place.root, place.projection.is_empty()) {
            (bir::PlaceRoot::Local(local), true) => Some(local.0),
            _ => None,
        }
    }

    fn operand(&mut self, operand: &bir::Operand) {
        match operand {
            bir::Operand::Constant(bir::Constant::Int(_) | bir::Constant::Bool(_) | bir::Constant::Float(_)) => {}
            bir::Operand::Constant(other) => {
                self.missing.insert(format!("constant {}", format!("{other:?}").split(['(', ' ']).next().unwrap_or("?")));
            }
            bir::Operand::Place(source) => {
                if !source.place.projection.is_empty() {
                    self.missing.insert("place projection (field/index)".to_string());
                }
                if matches!(source.place.root, bir::PlaceRoot::Global(_)) {
                    self.missing.insert("global place".to_string());
                }
                match source.fact {
                    bir::OwnershipFact::Copy | bir::OwnershipFact::Move => {}
                    bir::OwnershipFact::Borrow | bir::OwnershipFact::MutBorrow
                        if Census::local_of(&source.place).is_some_and(|l| self.ranges.contains(&l)) => {}
                    other => {
                        self.missing.insert(format!("operand fact {other:?}"));
                    }
                }
            }
        }
    }

    fn rvalue(&mut self, rvalue: &bir::Rvalue) {
        match rvalue {
            bir::Rvalue::Use(operand) | bir::Rvalue::UnaryOp(bir::UnOp::Not, operand) => self.operand(operand),
            bir::Rvalue::UnaryOp(op, operand) => {
                self.missing.insert(format!("unary {op:?}"));
                self.operand(operand);
            }
            bir::Rvalue::BinaryOp(op, a, b) => {
                use bir::BinOp::{Add, Div, Eq, FloorDiv, Ge, Gt, Le, Lt, Mod, Mul, Ne, Sub};
                if !matches!(op, Add | Sub | Mul | Div | Mod | FloorDiv | Eq | Ne | Lt | Le | Gt | Ge) {
                    self.missing.insert(format!("binary {op:?}"));
                }
                self.operand(a);
                self.operand(b);
            }
            bir::Rvalue::Format(parts) => {
                for part in parts {
                    match part {
                        bir::FormatPart::Literal(_) => {}
                        bir::FormatPart::Expr { operand, style } => {
                            if *style != bir::FormatStyle::Display {
                                self.missing.insert("f-string debug style".to_string());
                            }
                            self.operand(operand);
                        }
                    }
                }
            }
            other => {
                self.missing.insert(format!("rvalue {}", format!("{other:?}").split(['(', ' ', '{']).next().unwrap_or("?")));
            }
        }
    }

    fn call(&mut self, callee: &bir::Callee, args: &[bir::ArgumentElement]) {
        match callee {
            bir::Callee::Function(bir::CallableTarget::Named(target)) => match target.builtin {
                None | Some(BuiltinFnId::Range | BuiltinFnId::Float | BuiltinFnId::Print) => {}
                Some(other) => {
                    self.missing.insert(format!("builtin {other:?}"));
                }
            },
            bir::Callee::Function(bir::CallableTarget::Local(_)) => {
                self.missing.insert("call through a local callable".to_string());
            }
            bir::Callee::Method(target) => {
                self.missing.insert(format!("method call .{}()", format!("{target:?}").split("name: \"").nth(1).and_then(|s| s.split('"').next()).unwrap_or("?")));
            }
            bir::Callee::Helper(_) => {
                self.missing.insert("runtime helper call".to_string());
            }
            bir::Callee::ProviderOperation(_) => {
                self.missing.insert("provider operation".to_string());
            }
        }
        for arg in args {
            match arg {
                bir::ArgumentElement::One(operand) => self.operand(operand),
                _ => {
                    self.missing.insert("named or spread argument".to_string());
                }
            }
        }
    }

    fn block(&mut self, block: &bir::Block) {
        for stmt in &block.stmts {
            match &stmt.kind {
                bir::StatementKind::Assign { place, rvalue } => {
                    if !place.projection.is_empty() {
                        self.missing.insert("assignment to a field or index".to_string());
                    }
                    self.rvalue(rvalue);
                }
                bir::StatementKind::Call { callee, args, .. } => self.call(callee, args),
                bir::StatementKind::If { cond, then_block, else_block } => {
                    self.operand(cond);
                    self.block(then_block);
                    if let Some(block) = else_block {
                        self.block(block);
                    }
                }
                bir::StatementKind::Loop { body } => self.block(body),
                bir::StatementKind::Return { value } => {
                    if let Some(value) = value {
                        self.operand(value);
                    }
                }
                bir::StatementKind::Break { value: Some(_) } => {
                    self.missing.insert("break with a value".to_string());
                }
                bir::StatementKind::Break { value: None } | bir::StatementKind::Continue | bir::StatementKind::Drop { .. } | bir::StatementKind::Expr { .. } => {}
                bir::StatementKind::IterNext { iterator, .. } => {
                    let over_range = matches!(iterator, bir::Operand::Place(source) if Census::local_of(&source.place).is_some_and(|l| self.ranges.contains(&l)));
                    if !over_range {
                        self.missing.insert("for over a non-range iterable".to_string());
                    }
                }
                other => {
                    self.missing.insert(format!("statement {}", format!("{other:?}").split(['(', ' ', '{']).next().unwrap_or("?")));
                }
            }
        }
    }

    /// Locals holding a builtin `range`, and borrows of them, as the lowering plans them.
    fn plan_ranges(&mut self, block: &bir::Block) {
        for stmt in &block.stmts {
            match &stmt.kind {
                bir::StatementKind::Call { destination: Some(dest), callee: bir::Callee::Function(bir::CallableTarget::Named(target)), .. }
                    if target.builtin == Some(BuiltinFnId::Range) =>
                {
                    self.ranges.extend(Census::local_of(dest));
                }
                bir::StatementKind::Assign { place, rvalue: bir::Rvalue::Use(bir::Operand::Place(source)) }
                    if source.fact == bir::OwnershipFact::Borrow && Census::local_of(&source.place).is_some_and(|l| self.ranges.contains(&l)) =>
                {
                    self.ranges.extend(Census::local_of(place));
                }
                bir::StatementKind::If { then_block, else_block, .. } => {
                    self.plan_ranges(then_block);
                    if let Some(block) = else_block {
                        self.plan_ranges(block);
                    }
                }
                bir::StatementKind::Loop { body } => self.plan_ranges(body),
                _ => {}
            }
        }
    }

    fn run(body: &bir::Body) -> BTreeSet<String> {
        let mut census = Census { missing: BTreeSet::new(), ranges: BTreeSet::new(), body };
        census.plan_ranges(&body.block);
        for local in &census.body.locals {
            if !census.ranges.contains(&local.id.0)
                && let Some(key) = type_key(&local.ty)
            {
                census.missing.insert(key);
            }
        }
        if let Some(key) = type_key(&body.return_type) {
            census.missing.insert(format!("return {key}"));
        }
        if body.is_async {
            census.missing.insert("async function".to_string());
        }
        census.block(&body.block);
        census.missing
    }
}

fn check(source: &str) -> Option<bir::BodyIrModule> {
    let tokens = lexer::lex(source).ok()?;
    let program = parser::parse(&tokens).ok()?;
    let mut checker = TypeChecker::new();
    let module_path = vec!["census".to_string()];
    checker.set_current_module_path(Some(module_path.clone()));
    checker.check_program(&program).ok()?;
    Some(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
}

fn main() {
    let (mut files, mut unchecked, mut files_covered, mut bodies, mut covered) = (0, 0, 0, 0, 0);
    let mut blocking: BTreeMap<String, usize> = BTreeMap::new();
    let mut sole_blocker: BTreeMap<String, usize> = BTreeMap::new();
    for path in std::env::args().skip(1) {
        let Ok(source) = std::fs::read_to_string(&path) else { continue };
        files += 1;
        let Some(module) = check(&source) else {
            unchecked += 1;
            continue;
        };
        let mut file_covered = !module.bodies.is_empty();
        for body in &module.bodies {
            bodies += 1;
            let missing = Census::run(body);
            if missing.is_empty() {
                covered += 1;
            } else {
                file_covered = false;
            }
            if missing.len() == 1
                && let Some(only) = missing.iter().next()
            {
                *sole_blocker.entry(only.clone()).or_default() += 1;
            }
            for key in missing {
                *blocking.entry(key).or_default() += 1;
            }
        }
        if file_covered {
            files_covered += 1;
        }
    }
    println!("files: {files} ({unchecked} do not check standalone; {files_covered} fully covered)");
    println!("bodies: {bodies}; covered by the native lowering: {covered}");
    println!("\nmissing constructs, by bodies blocked (bodies where it is the only blocker):");
    let mut ranked: Vec<(&String, &usize)> = blocking.iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(a.1));
    for (key, count) in ranked.iter().take(40) {
        println!("{count:>6} ({:>4})  {key}", sole_blocker.get(*key).copied().unwrap_or(0));
    }
}

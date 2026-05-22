//! Serde derive and JSON activation planning for IR code generation.

use crate::frontend::ast::{Declaration, Program};
use crate::frontend::decorator_resolution;
use incan_core::lang::decorators::{self, DecoratorId};
use incan_core::lang::stdlib;

const SERDE_SERIALIZE_DERIVE: &str = "serde::Serialize";
const SERDE_DESERIALIZE_DERIVE: &str = "serde::Deserialize";

/// Return whether any loaded module derives serde serialize or deserialize through resolved JSON derive imports.
pub(super) fn collect_serde_derives(main: &Program, deps: &[(&str, &Program)]) -> (bool, bool) {
    let mut has_serialize = false;
    let mut has_deserialize = false;

    let mut visit = |program: &Program| {
        let import_aliases = decorator_resolution::collect_import_aliases(program);
        for decl in &program.declarations {
            let decorators = match &decl.node {
                Declaration::Model(m) => Some(&m.decorators),
                Declaration::Class(c) => Some(&c.decorators),
                Declaration::Enum(e) => Some(&e.decorators),
                _ => None,
            };
            let Some(decorators) = decorators else {
                continue;
            };
            for dec in decorators {
                if decorators::from_str(dec.node.name.as_str()) != Some(DecoratorId::Derive) {
                    continue;
                }
                for arg in &dec.node.args {
                    let crate::frontend::ast::DecoratorArg::Positional(expr) = arg else {
                        continue;
                    };
                    let crate::frontend::ast::Expr::Ident(name) = &expr.node else {
                        continue;
                    };
                    let resolved = import_aliases
                        .get(name)
                        .cloned()
                        .unwrap_or_else(|| vec![name.to_string()]);
                    match stdlib::stdlib_json_trait_id_from_path(&resolved) {
                        Some(stdlib::StdlibJsonTraitId::Serialize) => {
                            has_serialize = true;
                        }
                        Some(stdlib::StdlibJsonTraitId::Deserialize) => {
                            has_deserialize = true;
                        }
                        None if stdlib::is_stdlib_json_trait_module_path(&resolved) => {
                            has_serialize = true;
                            has_deserialize = true;
                        }
                        None => match resolved.as_slice() {
                            [serde, trait_name] if serde == "serde" && trait_name == "Serialize" => {
                                has_serialize = true;
                            }
                            [serde, trait_name] if serde == "serde" && trait_name == "Deserialize" => {
                                has_deserialize = true;
                            }
                            _ => {}
                        },
                    }
                }
            }
        }
    };

    visit(main);
    for (_, dep) in deps {
        visit(dep);
    }

    if !has_serialize && !has_deserialize {
        let serde_used = crate::backend::ir::scanners::detect_serde_usage(main)
            || deps
                .iter()
                .any(|(_, program)| crate::backend::ir::scanners::detect_serde_usage(program));
        if serde_used {
            has_serialize = true;
        }
    }

    (has_serialize, has_deserialize)
}

/// Add serde derives to generated newtypes when the current program needs serde support.
pub(super) fn add_serde_to_newtypes(
    ir_program: &mut crate::backend::ir::IrProgram,
    add_serialize: bool,
    add_deserialize: bool,
) {
    use crate::backend::ir::decl::IrDeclKind;
    use crate::backend::ir::types::IrType;

    fn is_conservative_serde_safe_newtype_inner(ty: &IrType) -> bool {
        match ty {
            IrType::Unit
            | IrType::Bool
            | IrType::Int
            | IrType::Float
            | IrType::String
            | IrType::Bytes
            | IrType::StaticStr
            | IrType::StaticBytes
            | IrType::FrozenStr
            | IrType::FrozenBytes
            | IrType::StrRef => true,
            IrType::List(inner) | IrType::Set(inner) | IrType::Option(inner) => {
                is_conservative_serde_safe_newtype_inner(inner)
            }
            IrType::Dict(key, value) | IrType::Result(key, value) => {
                is_conservative_serde_safe_newtype_inner(key) && is_conservative_serde_safe_newtype_inner(value)
            }
            IrType::Tuple(items) => items.iter().all(is_conservative_serde_safe_newtype_inner),
            _ => false,
        }
    }

    for decl in &mut ir_program.declarations {
        if let IrDeclKind::Struct(s) = &mut decl.kind
            && s.fields.len() == 1
            && s.fields[0].name == "0"
        {
            if !s.type_params.is_empty() {
                continue;
            }
            if !is_conservative_serde_safe_newtype_inner(&s.fields[0].ty) {
                continue;
            }
            if add_serialize && !s.derives.iter().any(|d| d == SERDE_SERIALIZE_DERIVE) {
                s.derives.push(SERDE_SERIALIZE_DERIVE.to_string());
            }
            if add_deserialize && !s.derives.iter().any(|d| d == SERDE_DESERIALIZE_DERIVE) {
                s.derives.push(SERDE_DESERIALIZE_DERIVE.to_string());
            }
        }
    }
}

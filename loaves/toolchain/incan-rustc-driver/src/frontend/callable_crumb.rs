//! Incan owns the callable product schema and checker rows. The Rust frontend owns checked declarations and Body IR.

use super::namespace_crumb::{CallWithInput, to_atom, to_cell};
use incan_frontend::declaration_interface::{CallableBodyProducts, CallableCheckerRows, CallableRowFamily};
use incan_semantics_core::CompilerNodeId;
use internal_models::CheckedFamily;

struct CheckerRows;

impl CallableCheckerRows for CheckerRows {
    fn write(&self, family: CallableRowFamily, fields: &[Vec<String>]) -> Result<Vec<u8>, String> {
        let rows = fields.to_vec();
        match family {
            CallableRowFamily::Declaration => internal_models::from_native_fields(&rows),
            CallableRowFamily::Local => internal_models::from_native_local_fields(&rows),
            CallableRowFamily::Final => internal_models::from_native_final_fields(&rows),
        }
        .and_then(|record| internal_models::encode_record(&record))
        .map_err(|error| format!("callable checker row: {error:?}"))
    }
    fn read(&self, family: CallableRowFamily, value: &[u8], owner: &str) -> Result<Vec<Vec<String>>, String> {
        let record = internal_models::decode_owned_record(
            &value.to_vec(),
            &incan_frontend::checker_records::GRAPH_BODY_RECORD_PROFILE.to_owned(),
            &owner.to_owned(),
        )
        .map_err(|error| format!("invalid callable checker row: {error:?}"))?;
        let expected = match family {
            CallableRowFamily::Declaration => CheckedFamily::Declaration,
            CallableRowFamily::Local => CheckedFamily::Local,
            CallableRowFamily::Final => CheckedFamily::Final,
        };
        if record.family != expected {
            return Err("callable checker row has a different family".into());
        }
        internal_models::callable_checker_fields.call_with_input(record)
            .map_err(|error| format!("invalid callable checker fields: {error:?}"))
    }
}

pub(super) fn encode(
    products: &CallableBodyProducts,
    module: &CompilerNodeId,
    source: &str,
) -> Result<Vec<u8>, String> {
    internal_models::encode_callable_inventory.call_with_input(to_atom(products.crumb_fields(
        module,
        source,
        &CheckerRows,
    )?))
}

pub(super) fn decode(bytes: &[u8], source: &str) -> Result<CallableBodyProducts, String> {
    if bytes.len() > 64 * 1024 * 1024 {
        return Err("callable inventory exceeds its byte limit".into());
    }
    let root = to_cell(internal_models::decode_callable_inventory(&bytes.to_vec())?)?;
    CallableBodyProducts::from_crumb_fields(root, source, &CheckerRows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use incan_frontend::body_ir::{BodyIrContext, ContextModules, ProviderOperationCatalog, build_body_ir_module};
    use incan_frontend::{lexer, parser, typechecker::TypeChecker};

    #[test]
    fn checked_callable_crumb_keeps_independent_owners_across_an_edit() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def first(x: int) -> int:\n    return x * 2\n\ndef second(x: int) -> int:\n    return x + 3\n\n";
        let parse = |text: &str| {
            let tokens = lexer::lex(text).map_err(|errors| format!("{errors:?}"))?;
            parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))
        };
        let package = "fixture";
        let modules = ["lib".to_owned()];
        let path = "src/lib.incn";
        let program = parse(source)?;
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(modules.to_vec()));
        checker.set_current_package_identity(Some(package.to_owned()));
        checker
            .check_program(&program)
            .map_err(|errors| format!("{errors:?}"))?;
        let providers = ProviderOperationCatalog::new();
        let body = build_body_ir_module(
            &program,
            &modules,
            checker.type_info(),
            &BodyIrContext {
                modules: ContextModules::default(),
                provider_operations: &providers,
                registry_package: None,
                library_package: Some(package),
                include_inline_tests: false,
            },
        );
        let products = CallableBodyProducts::from_checked(&program, source, &checker, &body)?;
        let encoded = encode(&products, &body.module_id, path)?;
        assert!(encoded.starts_with(b"CRMB"));
        let restored = decode(&encoded, path)?;
        assert_eq!(restored.native_inputs()?, products.native_inputs()?);
        assert_eq!(encode(&restored, &body.module_id, path)?, encoded);
        assert_eq!(restored.select(&program, source, package, &modules)?.len(), 2);
        let edited = source.replace("x * 2", "x * 7");
        assert_eq!(restored.select(&parse(&edited)?, &edited, package, &modules)?.len(), 1);
        assert!(decode(&encoded, "src/other.incn").is_err());
        assert!(decode(&products.to_bytes()?, path).is_err());
        let mut damaged = encoded;
        if let Some(last) = damaged.last_mut() {
            *last ^= 1;
        }
        assert!(decode(&damaged, path).is_err());
        Ok(())
    }
}

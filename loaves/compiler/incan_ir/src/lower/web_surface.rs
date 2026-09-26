//! Lowering of the `std.web` surface types into the Axum shapes they re-export (#1768).
//!
//! `std.web` re-exports Axum's extractor and response types directly (`loaves/stdlib/web/src/web/request.incn`,
//! `response.incn`), while the checker gives them an Incan surface (`incan_lang::lang::surface::types`): `Html` is a
//! plain named type, and `Json[T]` / `Query[T]` expose their payload as `.value`. The Rust types differ in exactly two
//! places, and this module is where lowering bridges them: Axum's `Html<T>` is generic over its body, and a `Json<T>`
//! or `Query<T>` stores its payload in the tuple field `0`.
//!
//! Both bridges apply only to the `std.web` bindings themselves, recognized by the checked import path a name was
//! bound through, under whatever local name the import chose. A type the program declares or imports from elsewhere
//! under one of those names keeps its own shape, whatever order the declarations come in.
//!
//! Known limitation (#1824): a module-qualified `web.Html` never reaches this module. `std.web` provides `Html` as a
//! compiler surface type backed by a Rust re-export, not as a declaration, so the checker's qualified resolution
//! cannot name it and refuses the spelling with a message that says so.

use incan_lang::lang::stdlib::{STDLIB_ROOT, STDLIB_WEB};
use incan_lang::lang::surface::types::{self as surface_types, SurfaceTypeId};

use super::super::types::IrType;
use super::AstLowering;

/// The surface field that names a web extractor's payload (`query.value`).
const WEB_EXTRACTOR_PAYLOAD_FIELD: &str = "value";

/// The Rust tuple field that holds an Axum extractor's payload.
const AXUM_EXTRACTOR_PAYLOAD_FIELD: &str = "0";

/// Return the `std.web` surface type a checked source path names, or `None` for any path outside `std.web`.
///
/// `path` is a module path followed by the member it names (`["std", "web", "Html"]`, or through the declaring
/// submodule, `["std", "web", "response", "Html"]`).
fn std_web_surface_type_at_path(path: &[String]) -> Option<SurfaceTypeId> {
    let [root, web, .., member] = path else {
        return None;
    };
    if root != STDLIB_ROOT || web != STDLIB_WEB {
        return None;
    }
    surface_types::from_str(member)
}

impl AstLowering {
    /// Return the `std.web` surface type a local type name is bound to, following an import alias, or `None` when the
    /// name is anything else.
    ///
    /// The checked import path is the authority: `from std.web import Html as Page` binds `Page` to
    /// `["std", "web", "Html"]`, while a model the program declares or a type imported from another module has no
    /// such path.
    fn std_web_surface_binding(&self, name: &str) -> Option<SurfaceTypeId> {
        let path = self.type_info.as_ref()?.import_binding_path(name)?;
        std_web_surface_type_at_path(path)
    }

    /// Return the Rust field and payload type a `Json[T].value` or `Query[T].value` read addresses, or `None` for
    /// any other field access.
    ///
    /// The checker resolves `.value` on those wrappers to the payload `T` (`check_expr/access.rs`); the Axum types
    /// hold it as their tuple field `0`, so the read is spelled `query.0` and typed `T`. Only the `std.web` bindings
    /// are rewritten: a `Json` or `Query` the program declares, before or after its use, or imports from another
    /// module keeps its own `value` field.
    pub(in crate::lower) fn web_extractor_payload_field(
        &self,
        object_ty: &IrType,
        field: &str,
    ) -> Option<(String, IrType)> {
        if field != WEB_EXTRACTOR_PAYLOAD_FIELD {
            return None;
        }
        let mut object_ty = object_ty;
        while let IrType::Ref(inner) | IrType::RefMut(inner) = object_ty {
            object_ty = inner.as_ref();
        }
        let IrType::NamedGeneric(name, args) = object_ty else {
            return None;
        };
        if !matches!(
            self.std_web_surface_binding(name),
            Some(SurfaceTypeId::Json | SurfaceTypeId::Query)
        ) {
            return None;
        }
        let [payload] = args.as_slice() else {
            return None;
        };
        Some((AXUM_EXTRACTOR_PAYLOAD_FIELD.to_string(), payload.clone()))
    }

    /// Return the Rust type of the `std.web` `Html` response when `name` is bound to it, or `None` otherwise.
    ///
    /// Axum's `Html<T>` is generic over its body, and the Incan surface `Html` wraps text, so the type is spelled
    /// `Html<String>` under the local name (`Page<String>` for `from std.web import Html as Page`); a bare `Html` in
    /// a signature is not a Rust type at all. A type the program declares under the same name, or imports from
    /// elsewhere, keeps its own spelling.
    pub(in crate::lower) fn std_web_html_type(&self, name: &str) -> Option<IrType> {
        (self.std_web_surface_binding(name) == Some(SurfaceTypeId::Html))
            .then(|| IrType::NamedGeneric(name.to_string(), vec![IrType::String]))
    }
}

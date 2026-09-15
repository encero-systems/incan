//! Programs checked by the frontend and then lowered, emitted or executed by the backend: the tests that need both
//! sides, kept beside the backend because the frontend sits below it.

mod borrowed_rust_enum;
mod embedded_fragment;
mod rust_supertrait_codegen;
mod sdk_module_derives;
mod tests;

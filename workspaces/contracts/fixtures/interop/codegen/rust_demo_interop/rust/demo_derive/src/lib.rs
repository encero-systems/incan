//! A derive that implements `demo::Component` for a struct, without depending on `syn`.

extern crate proc_macro;

use proc_macro::{TokenStream, TokenTree};

/// Derive `demo::Component` for the annotated struct.
#[proc_macro_derive(Component)]
pub fn derive_component(input: TokenStream) -> TokenStream {
    let mut tokens = input.into_iter();
    let mut name = None;
    while let Some(token) = tokens.next() {
        if let TokenTree::Ident(ident) = token {
            if ident.to_string() == "struct" {
                if let Some(TokenTree::Ident(struct_name)) = tokens.next() {
                    name = Some(struct_name.to_string());
                }
                break;
            }
        }
    }
    let name = name.unwrap_or_else(|| "Unnamed".to_string());
    format!("impl demo::Component for {name} {{}}")
        .parse()
        .unwrap_or_else(|_| TokenStream::new())
}

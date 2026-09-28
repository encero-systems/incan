use incan_frontend::ast::*;
use incan_frontend::{lexer, parser};

fn parse_str(source: &str) -> Result<Program, ()> {
    let tokens = lexer::lex(source).map_err(|_| ())?;
    parser::parse(&tokens).map_err(|_| ())
}

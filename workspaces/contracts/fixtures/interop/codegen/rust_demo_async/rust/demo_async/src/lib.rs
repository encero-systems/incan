//! Async Rust functions and methods taking references, the shapes the retired async codegen tests described.

/// A state passed by reference.
pub struct State {
    pub value: i64,
}

/// A plan passed by reference.
pub struct Plan {
    pub value: i64,
}

/// Consume both references asynchronously and return their sum.
pub async fn consume(state: &State, plan: &Plan) -> i64 {
    state.value + plan.value
}

/// Options for a registration.
pub struct CsvReadOptions {
    pub header: bool,
}

/// The error a registration can return.
pub struct RegistrationError;

/// A context whose registration is async and borrows `&self`.
pub struct SessionContext {
    pub registered: i64,
}

impl SessionContext {
    /// A fresh context.
    pub fn new() -> SessionContext {
        SessionContext { registered: 0 }
    }

    /// The state this context carries.
    pub fn state(&self) -> State {
        State { value: 5 }
    }

    /// Register a CSV source; an empty path is an error.
    pub async fn register_csv(&self, name: &str, path: &str, options: CsvReadOptions) -> Result<(), RegistrationError> {
        if name.is_empty() || path.is_empty() || !options.header {
            return Err(RegistrationError);
        }
        Ok(())
    }
}

impl Default for SessionContext {
    /// The `new` value.
    fn default() -> Self {
        SessionContext::new()
    }
}

/// Build a context.
pub fn make_context() -> SessionContext {
    SessionContext::new()
}

/// Build options with a header row.
pub fn make_options() -> CsvReadOptions {
    CsvReadOptions { header: true }
}

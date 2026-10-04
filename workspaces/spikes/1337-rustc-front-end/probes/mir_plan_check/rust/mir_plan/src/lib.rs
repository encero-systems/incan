//! The plan an Incan-written lowering fills: plain data with no rustc type, so the ordinary checker types it.
//! The driver, the one unit allowed rustc's internals, turns a finished plan into MIR.

/// Bodies the Incan side planned: each named function returns a constant.
#[derive(Debug, Clone, Default)]
pub struct BodyPlan {
    constants: Vec<(String, i64)>,
}

impl BodyPlan {
    /// An empty plan.
    pub fn new() -> BodyPlan {
        BodyPlan { constants: Vec::new() }
    }

    /// Plan `function`'s body as `return value`.
    pub fn add_constant_return(&mut self, function: &str, value: i64) {
        self.constants.push((function.to_string(), value));
    }

    /// How many bodies are planned.
    pub fn len(&self) -> i64 {
        self.constants.len() as i64
    }

    /// The constant planned for `function`, or -1 when none is.
    pub fn constant_for(&self, function: &str) -> i64 {
        self.constants.iter().find(|(name, _)| name == function).map(|(_, value)| *value).unwrap_or(-1)
    }
}

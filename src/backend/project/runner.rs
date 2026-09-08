//! Captured native command outcomes. Cargo execution has been removed.

/// Captured outcome of a native build.
#[derive(Debug)]
pub struct BuildResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Captured outcome of a fixture program.
#[cfg(test)]
#[derive(Debug)]
pub struct RunResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

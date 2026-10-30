//! Scores how well agents can use an MCP server: static lint plus Claude task runs.

mod config;
mod lint;
mod mcp;
mod model;
mod run;

pub use config::{DEFAULT_MAX_TURNS, DEFAULT_MODEL, Expect, ServerConfig, Task, TaskFile};
pub use lint::{Finding, LintReport, Severity, ToolCost, lint, lint_tools};
pub use mcp::{Annotations, Session, ToolDef, ToolOutput};
pub use model::{HttpModel, Model, ModelRequest, ModelResponse, Usage};
pub use run::{RunReport, Summary, TaskReport, ToolCallRecord, run};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("config: {0}")]
    Config(String),
    #[error("mcp: {0}")]
    Mcp(String),
    #[error("model: {0}")]
    Model(String),
}

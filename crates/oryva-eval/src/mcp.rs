use std::time::Duration;

use rmcp::{
    RoleClient, ServiceExt,
    model::{CallToolRequestParams, ContentBlock},
    service::RunningService,
    transport::TokioChildProcess,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::process::Command;

use crate::{Error, ServerConfig};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(180);
const CALL_TIMEOUT: Duration = Duration::from_secs(120);

/// A tool as the server advertises it (the subset oryva cares about).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDef {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub input_schema: Value,
    #[serde(default)]
    pub annotations: Option<Annotations>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Annotations {
    pub read_only_hint: Option<bool>,
    pub destructive_hint: Option<bool>,
}

/// What came back from a tool call.
#[derive(Debug, Clone)]
pub struct ToolOutput {
    pub text: String,
    pub is_error: bool,
}

/// A live stdio connection to an MCP server. The child process is killed on drop.
pub struct Session {
    service: RunningService<RoleClient, ()>,
}

impl Session {
    pub async fn connect(cfg: &ServerConfig) -> Result<Self, Error> {
        let mut cmd = Command::new(&cfg.command);
        cmd.args(&cfg.args).envs(&cfg.env);
        let transport = TokioChildProcess::new(cmd)
            .map_err(|e| Error::Mcp(format!("spawning `{}`: {e}", cfg.command)))?;
        let service = tokio::time::timeout(CONNECT_TIMEOUT, ().serve(transport))
            .await
            .map_err(|_| Error::Mcp(format!("timed out connecting to `{}`", cfg.command)))?
            .map_err(|e| Error::Mcp(format!("initializing `{}`: {e}", cfg.command)))?;
        Ok(Self { service })
    }

    pub async fn tools(&self) -> Result<Vec<ToolDef>, Error> {
        let tools = self
            .service
            .list_all_tools()
            .await
            .map_err(|e| Error::Mcp(format!("list_tools: {e}")))?;
        tools
            .iter()
            .map(|t| {
                serde_json::to_value(t)
                    .and_then(serde_json::from_value)
                    .map_err(|e| Error::Mcp(e.to_string()))
            })
            .collect()
    }

    /// Calls a tool. A protocol failure is reported as an error output, like the tool's own
    /// `isError`, because the model should see both.
    pub async fn call(&self, name: &str, args: Option<Map<String, Value>>) -> ToolOutput {
        let mut params = CallToolRequestParams::new(name.to_string());
        params.arguments = args;
        match tokio::time::timeout(CALL_TIMEOUT, self.service.call_tool(params)).await {
            Err(_) => ToolOutput {
                text: format!("tool call timed out after {}s", CALL_TIMEOUT.as_secs()),
                is_error: true,
            },
            Ok(Err(e)) => ToolOutput {
                text: format!("MCP protocol error: {e}"),
                is_error: true,
            },
            Ok(Ok(res)) => ToolOutput {
                text: res
                    .content
                    .iter()
                    .map(block_text)
                    .collect::<Vec<_>>()
                    .join("\n"),
                is_error: res.is_error.unwrap_or(false),
            },
        }
    }

    pub async fn close(mut self) {
        let _ = self.service.close().await;
    }
}

fn block_text(block: &ContentBlock) -> String {
    match block.as_text() {
        Some(t) => t.text.clone(),
        None => {
            let kind = serde_json::to_value(block)
                .ok()
                .and_then(|v| v.get("type").and_then(Value::as_str).map(str::to_owned))
                .unwrap_or_else(|| "unknown".into());
            format!("[non-text content: {kind}]")
        }
    }
}

//! Tiny MCP server (stdio) with one well-behaved tool and three deliberately flawed ones.

use std::sync::atomic::{AtomicUsize, Ordering};

use rmcp::{
    ErrorData, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::CallToolResult,
    schemars, tool, tool_handler, tool_router,
    transport::stdio,
};
use serde::Deserialize;

#[derive(Deserialize, schemars::JsonSchema)]
struct EchoArgs {
    /// Text to send back unchanged.
    message: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct DeleteArgs {
    // Deliberately undocumented so lint flags it.
    id: String,
}

#[derive(Clone)]
struct TestServer {
    flaky_calls: std::sync::Arc<AtomicUsize>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl TestServer {
    fn new() -> Self {
        Self {
            flaky_calls: Default::default(),
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Returns the given message unchanged. Use it to check that the connection works.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn echo(
        &self,
        Parameters(args): Parameters<EchoArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(CallToolResult::success(vec![
            rmcp::model::ContentBlock::text(args.message),
        ]))
    }

    #[tool(description = "Deletes a thing.")]
    async fn delete_thing(
        &self,
        Parameters(args): Parameters<DeleteArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(CallToolResult::success(vec![
            rmcp::model::ContentBlock::text(format!("deleted {}", args.id)),
        ]))
    }

    #[tool(
        description = "Fails on the first call and succeeds on every call after that, to test retries.",
        annotations(read_only_hint = true)
    )]
    async fn flaky(&self) -> Result<CallToolResult, ErrorData> {
        let content = if self.flaky_calls.fetch_add(1, Ordering::SeqCst) == 0 {
            CallToolResult::error(vec![rmcp::model::ContentBlock::text(
                "temporary failure, try again",
            )])
        } else {
            CallToolResult::success(vec![rmcp::model::ContentBlock::text("flaky ok")])
        };
        Ok(content)
    }

    #[tool(
        description = "This tool is retired and will be removed. Do not use it.",
        annotations(read_only_hint = true)
    )]
    async fn retired_tool(&self) -> Result<CallToolResult, ErrorData> {
        Ok(CallToolResult::error(vec![
            rmcp::model::ContentBlock::text("this tool has been retired"),
        ]))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for TestServer {}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    TestServer::new().serve(stdio()).await?.waiting().await?;
    Ok(())
}

use rmcp::{
    model::{self, Content, ServerInfo, ServerCapabilities, ToolsCapability, Implementation, CallToolRequestParam, CallToolResult, CallToolRequestMethod},
    ServiceExt,
    ServerHandler,
    service::RequestContext,
    RoleServer,
    Error as McpError,
};
use std::{sync::Arc, future::Future};
use rmcp::serde_json::Map;
use tokio::{
    io::{stdin, stdout},
    process::Command,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("mc-mcp server (MCP over stdio) started.");

    let handler = MyHandler {};

    let transport = (stdin(), stdout());

    let server_handle = handler.serve(transport).await?;

    let shutdown_reason = server_handle.waiting().await?;
    println!("mc-mcp server finished. Reason: {:?}", shutdown_reason);

    Ok(())
}

#[derive(Clone)]
struct MyHandler;

impl MyHandler {
    async fn forge_test(&self) -> Result<Vec<Content>, String> {
        println!("Executing forge test...");
        let output_result = Command::new("forge")
            .arg("test")
            .output()
            .await;

        match output_result {
            Ok(output) => {
                println!("Forge test finished.");
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let status_code = output.status.code().map_or("N/A".to_string(), |c| c.to_string());

                let result_text = format!(
                    "Forge Test Results:\nExit Code: {}\n\nStdout:\n{}\nStderr:\n{}",
                    status_code,
                    stdout,
                    stderr
                );

                Ok(vec![Content::text(result_text)])
            }
            Err(e) => {
                println!("Failed to execute forge test: {}", e);
                Err(format!("Failed to execute forge command: {}", e))
            }
        }
    }
}

impl ServerHandler for MyHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            server_info: Implementation {
                name: "mc-mcp-server".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            capabilities: ServerCapabilities {
                tools: Some(ToolsCapability {
                    list_changed: None,
                }),
                experimental: None,
                logging: None,
                prompts: None,
                resources: None,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn call_tool(
        &self,
        params: CallToolRequestParam,
        _ctx: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResult, McpError>> + Send + '_ {
        async move {
            match params.name.as_ref() {
                "forge_test" => {
                    let handler_clone = self.clone();
                    match handler_clone.forge_test().await {
                        Ok(content) => Ok(CallToolResult::success(content)),
                        Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
                    }
                }
                _ => Err(McpError::method_not_found::<CallToolRequestMethod>()),
            }
        }
    }
}

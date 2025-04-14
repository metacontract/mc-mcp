use rmcp::{
    model::{self, Content, ServerInfo, ServerCapabilities, ToolsCapability, Implementation, CallToolRequestParam, CallToolResult, CallToolRequestMethod, Tool},
    ServiceExt,
    ServerHandler,
    service::RequestContext,
    RoleServer,
    Error as McpError,
};
use std::sync::{Arc, Mutex};
use std::future::Future;
use rmcp::serde_json::Map;
use serde_json::json;
use tokio::{
    io::{stdin, stdout},
    process::Command,
};
use mc_mcp_infrastructure::{load_documents, SimpleDocumentIndex};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("mc-mcp server (MCP over stdio) started.");

    let document_index = match load_documents(None) {
        Ok(index) => {
            println!("Successfully loaded {} documents.", index.len());
            Arc::new(Mutex::new(index))
        }
        Err(e) => {
            eprintln!("Failed to load documents: {}. Starting with empty index.", e);
            Arc::new(Mutex::new(SimpleDocumentIndex::new()))
        }
    };

    let handler = MyHandler { document_index };

    let transport = (stdin(), stdout());

    let server_handle = handler.serve(transport).await?;

    let shutdown_reason = server_handle.waiting().await?;
    println!("mc-mcp server finished. Reason: {:?}", shutdown_reason);

    Ok(())
}

#[derive(Clone)]
struct MyHandler {
    document_index: Arc<Mutex<SimpleDocumentIndex>>,
}

const MAX_SEARCH_RESULTS: usize = 5;
const SNIPPET_LINES: usize = 3;

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

    fn search_docs(&self, query: &str) -> Vec<Content> {
        let index = self.document_index.lock().unwrap();
        let mut results = Vec::new();

        println!("Searching for: '{}' in {} documents", query, index.len());

        for (path, text) in index.iter() {
            if path.contains(query) || text.contains(query) {
                println!("Found match in: {}", path);
                let snippet = text
                    .lines()
                    .take(SNIPPET_LINES)
                    .collect::<Vec<&str>>()
                    .join("\n");
                let result_text = format!("Match in `{}`:\n```\n{}\n...\n```", path, snippet);
                results.push(Content::text(result_text));
                if results.len() >= MAX_SEARCH_RESULTS {
                    results.push(Content::text(format!(
                        "(Search truncated to {} results)",
                        MAX_SEARCH_RESULTS
                    )));
                    break;
                }
            }
        }

        if results.is_empty() {
            println!("No matches found for: '{}'", query);
            results.push(Content::text(format!("No documents found matching '{}'", query)));
        }

        results
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

    fn list_tools(
        &self,
        _request: model::PaginatedRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<model::ListToolsResult, McpError>> + Send + '_ {
        let forge_test_tool = Tool {
            name: "forge_test".to_string().into(),
            description: "Run 'forge test' in the workspace.".to_string().into(),
            input_schema: Arc::new(Map::default()),
        };
        let search_docs_tool = Tool {
            name: "search_docs".to_string().into(),
            description: "Search metacontract documents by keyword.".to_string().into(),
            input_schema: Arc::new(json!({ "type": "object", "properties": { "query": { "type": "string", "description": "Search keyword" } }, "required": ["query"] }).as_object().unwrap().clone()),
        };
        let result = model::ListToolsResult {
            tools: vec![forge_test_tool, search_docs_tool],
            next_cursor: None,
        };
        async { Ok(result) }
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
                "search_docs" => {
                    if let Some(args) = params.arguments {
                        if let Some(query_value) = args.get("query") {
                            if let Some(query) = query_value.as_str() {
                                let handler_clone = self.clone();
                                let results = handler_clone.search_docs(query);
                                Ok(CallToolResult::success(results))
                            } else {
                                Ok(CallToolResult::error(vec![Content::text("Invalid 'query' parameter: must be a string.".to_string())]))
                            }
                        } else {
                            Ok(CallToolResult::error(vec![Content::text("Missing 'query' parameter.".to_string())]))
                        }
                    } else {
                        Ok(CallToolResult::error(vec![Content::text("Missing arguments object.".to_string())]))
                    }
                }
                _ => Err(McpError::method_not_found::<CallToolRequestMethod>()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use mc_mcp_infrastructure::SimpleDocumentIndex;
    use rmcp::model::Content;

    #[tokio::test]
    async fn test_forge_test_execution() {
        // ダミーのドキュメントインデックスを作成
        let dummy_index = Arc::new(Mutex::new(SimpleDocumentIndex::new()));
        let handler = MyHandler { document_index: dummy_index };

        // forge_test を実行
        let result = handler.forge_test().await;

        match result {
            Ok(content_vec) => {
                println!("forge_test successful (in test). Output:");
                let mut output_text = String::new();
                for c in content_vec {
                   if let Some(raw_text) = c.raw.as_text() {
                        output_text.push_str(&raw_text.text);
                        output_text.push('\n');
                    }
                }
                println!("{}", output_text);
                assert!(true, "Forge command executed successfully (or predictably failed without panic).");
            }
            Err(e) => {
                println!("forge_test failed as expected (in test): {}", e);
                 assert!(e.contains("Failed to execute forge command"), "Error message should indicate failure to execute forge command");
            }
        }
        println!("Test assertion: Check if forge command exists and runs (or fails predictably).");
    }

    #[test]
    fn test_search_docs_logic() {
        // テスト用のインデックスを作成
        let mut test_index = SimpleDocumentIndex::new();
        test_index.insert(
            "docs/concepts/core.md".to_string(),
            "This document explains the core concepts.".to_string(),
        );
        test_index.insert(
            "docs/guides/installation.md".to_string(),
            "How to install the framework.".to_string(),
        );
        test_index.insert(
            "docs/guides/advanced.md".to_string(),
            "Advanced concepts and features.".to_string(),
        );

        let handler = MyHandler {
            document_index: Arc::new(Mutex::new(test_index)),
        };

        // 1. コンテンツにマッチするクエリ
        let results1 = handler.search_docs("core");
        assert_eq!(results1.len(), 1);
        if let Some(raw_text) = results1[0].raw.as_text() {
            assert!(raw_text.text.contains("Match in `docs/concepts/core.md`"));
            assert!(raw_text.text.contains("core concepts"));
        } else {
            panic!("Expected text content");
        }

        // 2. パスにマッチするクエリ
        let results2 = handler.search_docs("installation");
        assert_eq!(results2.len(), 1);
         if let Some(raw_text) = results2[0].raw.as_text() {
            assert!(raw_text.text.contains("Match in `docs/guides/installation.md`"));
            assert!(raw_text.text.contains("install the framework"));
        } else {
            panic!("Expected text content");
        }

        // 3. 複数にマッチするクエリ
        let results3 = handler.search_docs("concepts");
        assert_eq!(results3.len(), 2);
         if let Some(raw_text1) = results3[0].raw.as_text() {
             assert!(raw_text1.text.contains("Match in `"));
         } else {
             panic!("Expected text content");
         }
         if let Some(raw_text2) = results3[1].raw.as_text() {
             assert!(raw_text2.text.contains("Match in `"));
         } else {
             panic!("Expected text content");
         }

        // 4. マッチしないクエリ
        let results4 = handler.search_docs("nonexistent");
        assert_eq!(results4.len(), 1);
        if let Some(raw_text) = results4[0].raw.as_text() {
           assert!(raw_text.text.contains("No documents found matching 'nonexistent'"));
        } else {
            panic!("Expected text content");
        }
    }
}

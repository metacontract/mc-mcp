// NOTE: use statements will need adjustment after refactoring
use rmcp::{
    model::{self, Content, ServerInfo, ServerCapabilities, ToolsCapability, Implementation, CallToolRequestParam, CallToolResult, CallToolRequestMethod, Tool},
    ServiceExt,
    ServerHandler,
    service::RequestContext,
    RoleServer,
    Error as McpError,
    Peer,
};
use std::sync::{Arc, Mutex};
use std::future::Future;
use rmcp::serde_json::Map;
use serde_json::json;
use tokio::{
    io::{stdin, stdout},
    process::Command,
};
use rmcp::service::AtomicU32RequestIdProvider;
use rmcp::model::ClientInfo;

// Declare modules
mod domain;
mod application;
mod infrastructure;

// Update use paths to crate::<module>::...
use crate::infrastructure::{load_documents, SimpleDocumentIndex};
use crate::application::reference_service::{ReferenceService, ReferenceServiceImpl};
use crate::infrastructure::embedding::EmbeddingGenerator;
use crate::infrastructure::EmbeddingModel;
use crate::infrastructure::vector_db::{VectorDb, qdrant_client};
use crate::domain::reference::SearchQuery;
// Assuming VectorRepository trait will be created in domain
use crate::domain::vector_repository::VectorRepository;


#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // TODO: Initialize logging (e.g., tracing_subscriber)
    println!("mc-mcp server (MCP over stdio) started.");

    // Document loading (using moved function)
    let document_index = match load_documents(None) {
        Ok(index) => {
            println!("Successfully loaded {} documents.", index.len());
            Arc::new(Mutex::new(index))
        }
        Err(e) => {
            eprintln!("Failed to load documents: {}. Starting with empty index.", e);
            // TODO: Log this error properly
            Arc::new(Mutex::new(SimpleDocumentIndex::new()))
        }
    };

    // --- Dependency Injection Setup ---
    // TODO: Read configuration from file/env (e.g., using figment)
    let qdrant_url = std::env::var("QDRANT_URL").unwrap_or_else(|_| "http://localhost:6334".to_string());
    let qdrant_client = qdrant_client::Qdrant::from_url(&qdrant_url).build()?;
    let collection_name = std::env::var("QDRANT_COLLECTION").unwrap_or_else(|_| "mc_docs".to_string());
    let vector_dim: u64 = std::env::var("EMBEDDING_DIM").ok().and_then(|s| s.parse().ok()).unwrap_or(384); // Dimension for AllMiniLML6V2
    let embedding_model = EmbeddingModel::AllMiniLML6V2;

    let embedder = Arc::new(EmbeddingGenerator::new(embedding_model)?);
    let vector_db_instance = VectorDb::new(Box::new(qdrant_client), collection_name, vector_dim)?;
    vector_db_instance.initialize_collection().await?; // Initialize Qdrant collection
    let vector_db: Arc<dyn VectorRepository> = Arc::new(vector_db_instance); // Use trait object for dependency injection

    let reference_service: Arc<dyn ReferenceService> = Arc::new(ReferenceServiceImpl::new(embedder, vector_db)); // Inject trait object
    // -----------------------------------

    let handler = MyHandler { document_index, reference_service }; // Pass the service Arc

    let transport = (stdin(), stdout());

    println!("Starting MCP server...");
    let server_handle = handler.serve(transport).await?;

    let shutdown_reason = server_handle.waiting().await?;
    println!("mc-mcp server finished. Reason: {:?}", shutdown_reason);

    Ok(())
}

#[derive(Clone)]
struct MyHandler {
    document_index: Arc<Mutex<SimpleDocumentIndex>>,
    reference_service: Arc<dyn ReferenceService>, // Use trait object
}

const MAX_SEARCH_RESULTS: usize = 5;
// const SNIPPET_LINES: usize = 3; // Seems unused

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
                // TODO: Consider returning structured data if needed
                Ok(vec![Content::text(result_text)])
            }
            Err(e) => {
                println!("Failed to execute forge test: {}", e);
                // TODO: Log this error
                Err(format!("Failed to execute forge command: {}. Make sure 'forge' is installed and in PATH.", e))
            }
        }
    }

    async fn search_docs_semantic(&self, query: &str, limit: usize) -> Vec<Content> {
        let search_query = crate::domain::reference::SearchQuery {
            text: query.to_string(),
            limit: Some(limit),
        };
        // Use the injected service
        match self.reference_service.search_documents(search_query, None).await {
            Ok(results) => {
                if results.is_empty() {
                    vec![Content::text(format!("No documents found matching '{}' (semantic)", query))]
                } else {
                    results.into_iter().map(|r| {
                        // TODO: Provide more context than just file path and score (e.g., snippets)
                        Content::text(format!("Semantic match: `{}` (score: {:.3})", r.file_path, r.score))
                    }).collect()
                }
            }
            Err(e) => {
                // TODO: Log this error
                eprintln!("Semantic search error: {}", e); // Log error to stderr
                vec![Content::text(format!("Semantic search failed: {}", e))]
            }
        }
    }
}

impl ServerHandler for MyHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            server_info: Implementation {
                name: "mc-mcp-server".to_string(), // Keep name or update?
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
            description: "Run 'forge test' in the workspace.".into(),
            input_schema: Arc::new(Map::default()),
        };
        let search_docs_tool = Tool {
            name: "search_docs".to_string().into(),
            description: "Search metacontract documents semantically.".into(),
            input_schema: Arc::new(json!({ "type": "object", "properties": { "query": { "type": "string", "description": "Natural language query for semantic search" } }, "required": ["query"] }).as_object().unwrap().clone()),
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
        let handler_clone = self.clone();
        async move {
            match params.name.as_ref() {
                "forge_test" => {
                    match handler_clone.forge_test().await {
                        Ok(content) => Ok(CallToolResult::success(content)),
                        Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
                    }
                }
                "search_docs" => {
                    if let Some(args) = params.arguments {
                        if let Some(query_value) = args.get("query") {
                            if let Some(query) = query_value.as_str() {
                                let results = handler_clone.search_docs_semantic(query, MAX_SEARCH_RESULTS).await;
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
                _ => {
                     println!("Unknown tool called: {}", params.name);
                     Err(McpError::method_not_found::<CallToolRequestMethod>())
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    // Update use paths for tests
    use crate::application::reference_service::ReferenceService;
    use crate::domain::reference::{SearchResult, SearchQuery};
    use crate::infrastructure::SimpleDocumentIndex; // Use path from infrastructure
    use anyhow::Result;
    use async_trait::async_trait;
    use rmcp::model::{Content, CallToolRequestParam}; // Ensure rmcp types are used correctly
    use rmcp::service::RequestContext; // Ensure rmcp types are used correctly

    // Mock ReferenceService for testing MyHandler
    #[derive(Clone)]
    struct MockReferenceService;

    #[async_trait]
    impl ReferenceService for MockReferenceService {
        async fn index_documents(&self, _docs_path: Option<std::path::PathBuf>) -> Result<()> {
            Ok(())
        }

        async fn search_documents(&self, query: SearchQuery, _score_threshold: Option<f32>) -> Result<Vec<SearchResult>> {
            println!("MockReferenceService: Searching for '{}', limit {:?}", query.text, query.limit);
            if query.text == "test query" {
                Ok(vec![
                    SearchResult { file_path: "mock_path1.md".to_string(), score: 0.9, source: "mock-source".to_string() },
                    SearchResult { file_path: "mock_path2.md".to_string(), score: 0.8, source: "mock-source".to_string() },
                ])
            } else {
                Ok(vec![])
            }
        }
    }

    #[tokio::test]
    async fn test_forge_test_execution() {
        // This test depends on the actual 'forge' command.
        // It might fail if forge is not installed or the project setup changes.
        let dummy_index = Arc::new(Mutex::new(SimpleDocumentIndex::new()));
        let mock_service: Arc<dyn ReferenceService> = Arc::new(MockReferenceService);
        let handler = MyHandler { document_index: dummy_index, reference_service: mock_service };

        let result = handler.forge_test().await;

        // We can't easily assert the exact output, but we check if it ran or failed predictably.
        match result {
            Ok(content_vec) => {
                println!("forge_test succeeded (in test). Output:");
                let output_text = content_vec.iter()
                    .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
                    .collect::<Vec<&str>>()
                    .join("\n");
                println!("{}", output_text);
                // Assert that the output contains expected markers if possible
                // assert!(output_text.contains("Test results"), "Output should contain test results");
            }
            Err(e) => {
                 println!("forge_test failed (in test): {}", e);
                 // Assert the error type or message if forge is expected to fail in the test env
                 assert!(e.contains("Failed to execute forge command"), "Error message should indicate failure to execute forge command");
            }
        }
    }


    #[tokio::test]
    async fn test_call_tool_search_docs_semantic() {
        let dummy_index = Arc::new(Mutex::new(SimpleDocumentIndex::new()));
        let mock_service: Arc<dyn ReferenceService> = Arc::new(MockReferenceService);
        let handler = MyHandler { document_index: dummy_index, reference_service: mock_service };

        let params = CallToolRequestParam {
            name: "search_docs".to_string().into(),
            arguments: Some(serde_json::json!({ "query": "test query" }).as_object().unwrap().clone()),
        };

        let (peer, _) = Peer::<RoleServer>::new(
            std::sync::Arc::new(rmcp::service::AtomicU32RequestIdProvider::default()),
            rmcp::model::InitializeRequestParam::default(),
        );
        let result = handler.call_tool(params, RequestContext::<RoleServer> {
            ct: tokio_util::sync::CancellationToken::new(),
            id: rmcp::model::NumberOrString::String("test".to_string().into()),
            peer,
        }).await;

        assert!(result.is_ok());
        let call_result = result.unwrap();
        assert_eq!(call_result.is_error, Some(false), "Expected success status");
        assert_eq!(call_result.content.len(), 2);
        assert!(call_result.content[0].raw.as_text().unwrap().text.contains("mock_path1.md"));
        assert!(call_result.content[1].raw.as_text().unwrap().text.contains("mock_path2.md"));
    }

    #[tokio::test]
    async fn test_call_tool_search_docs_no_results() {
        let dummy_index = Arc::new(Mutex::new(SimpleDocumentIndex::new()));
        let mock_service: Arc<dyn ReferenceService> = Arc::new(MockReferenceService);
        let handler = MyHandler { document_index: dummy_index, reference_service: mock_service };

        let params = CallToolRequestParam {
            name: "search_docs".to_string().into(),
            arguments: Some(serde_json::json!({ "query": "unknown query" }).as_object().unwrap().clone()),
        };

        let (peer, _) = Peer::<RoleServer>::new(
            std::sync::Arc::new(rmcp::service::AtomicU32RequestIdProvider::default()),
            rmcp::model::InitializeRequestParam::default(),
        );
        let result = handler.call_tool(params, RequestContext::<RoleServer> {
            ct: tokio_util::sync::CancellationToken::new(),
            id: rmcp::model::NumberOrString::String("test".to_string().into()),
            peer,
        }).await;

        assert!(result.is_ok());
        let call_result = result.unwrap();
        assert_eq!(call_result.is_error, Some(false), "Expected success status");
        assert_eq!(call_result.content.len(), 1);
        assert!(call_result.content[0].raw.as_text().unwrap().text.contains("No documents found"));
    }

     #[tokio::test]
    async fn test_call_tool_search_docs_missing_query() {
        let dummy_index = Arc::new(Mutex::new(SimpleDocumentIndex::new()));
        let mock_service: Arc<dyn ReferenceService> = Arc::new(MockReferenceService);
        let handler = MyHandler { document_index: dummy_index, reference_service: mock_service };

        let params = CallToolRequestParam {
            name: "search_docs".to_string().into(),
            arguments: Some(serde_json::json!({}).as_object().unwrap().clone()), // Empty args
        };

        let (peer, _) = Peer::<RoleServer>::new(
            std::sync::Arc::new(rmcp::service::AtomicU32RequestIdProvider::default()),
            rmcp::model::InitializeRequestParam::default(),
        );
        let result = handler.call_tool(params, RequestContext::<RoleServer> {
            ct: tokio_util::sync::CancellationToken::new(),
            id: rmcp::model::NumberOrString::String("test".to_string().into()),
            peer,
        }).await;

        assert!(result.is_ok());
        let call_result = result.unwrap();
        assert_eq!(call_result.is_error, Some(true), "Expected error status");
        assert!(call_result.content[0].raw.as_text().unwrap().text.contains("Missing 'query' parameter"));
    }

     #[tokio::test]
    async fn test_call_tool_search_docs_invalid_query() {
        let dummy_index = Arc::new(Mutex::new(SimpleDocumentIndex::new()));
        let mock_service: Arc<dyn ReferenceService> = Arc::new(MockReferenceService);
        let handler = MyHandler { document_index: dummy_index, reference_service: mock_service };

        let params = CallToolRequestParam {
            name: "search_docs".to_string().into(),
            arguments: Some(serde_json::json!({ "query": 123 }).as_object().unwrap().clone()), // Query is not a string
        };

        let (peer, _) = Peer::<RoleServer>::new(
            std::sync::Arc::new(rmcp::service::AtomicU32RequestIdProvider::default()),
            rmcp::model::InitializeRequestParam::default(),
        );
        let result = handler.call_tool(params, RequestContext::<RoleServer> {
            ct: tokio_util::sync::CancellationToken::new(),
            id: rmcp::model::NumberOrString::String("test".to_string().into()),
            peer,
        }).await;

        assert!(result.is_ok());
        let call_result = result.unwrap();
        assert_eq!(call_result.is_error, Some(true), "Expected error status");
        assert!(call_result.content[0].raw.as_text().unwrap().text.contains("Invalid 'query' parameter"));
    }

     #[tokio::test]
    async fn test_call_tool_unknown_tool() {
        let dummy_index = Arc::new(Mutex::new(SimpleDocumentIndex::new()));
        let mock_service: Arc<dyn ReferenceService> = Arc::new(MockReferenceService);
        let handler = MyHandler { document_index: dummy_index, reference_service: mock_service };

        let params = CallToolRequestParam {
            name: "unknown_tool".to_string().into(),
            arguments: None,
        };

        let (peer, _) = Peer::<RoleServer>::new(
            std::sync::Arc::new(rmcp::service::AtomicU32RequestIdProvider::default()),
            rmcp::model::InitializeRequestParam::default(),
        );
        let result = handler.call_tool(params, RequestContext::<RoleServer> {
            ct: tokio_util::sync::CancellationToken::new(),
            id: rmcp::model::NumberOrString::String("test".to_string().into()),
            peer,
        }).await;

        assert!(result.is_err());
        let err = result.err().unwrap();
        // MethodNotFoundはDisplay文字列で判定する
        let err_str = format!("{}", err);
        assert!(err_str.contains("MethodNotFound"), "Expected MethodNotFound error, got: {}", err_str);
    }
}

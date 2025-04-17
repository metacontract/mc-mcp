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
mod config;
mod application;
mod infrastructure;

// Update use paths to crate::<module>::...
use crate::infrastructure::{load_documents, load_prebuilt_index, SimpleDocumentIndex};
use crate::application::reference_service::{ReferenceService, ReferenceServiceImpl};
use crate::infrastructure::embedding::EmbeddingGenerator;
use crate::infrastructure::EmbeddingModel;
use crate::infrastructure::vector_db::{VectorDb, qdrant_client};
use crate::domain::reference::SearchQuery;
// Assuming VectorRepository trait will be created in domain
use crate::domain::vector_repository::VectorRepository;
use crate::config::{self, McpConfig}; // Add config module


#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // TODO: Initialize logging (e.g., tracing_subscriber)
    println!("mc-mcp server (MCP over stdio) started.");

    // --- Load Configuration ---
    let config = config::load_config().map_err(|e| {
        eprintln!("Failed to load configuration: {}", e);
        // Provide a more specific error type if needed
        Box::new(e) as Box<dyn std::error::Error>
    })?;
    println!("Configuration loaded: {:?}", config);

    // --- Dependency Injection Setup ---
    // Use values from config where applicable, keep env var overrides for now
    let qdrant_url = std::env::var("QDRANT_URL").unwrap_or_else(|_| "http://localhost:6334".to_string());
    let qdrant_client = qdrant_client::Qdrant::from_url(&qdrant_url).build()?;
    let collection_name = std::env::var("QDRANT_COLLECTION").unwrap_or_else(|_| "mc_docs".to_string());
    let vector_dim: u64 = std::env::var("EMBEDDING_DIM").ok().and_then(|s| s.parse().ok()).unwrap_or(384); // Dimension for AllMiniLML6V2
    let embedding_model = EmbeddingModel::AllMiniLML6V2;

    let embedder = Arc::new(EmbeddingGenerator::new(embedding_model)?);
    let vector_db_instance = VectorDb::new(Box::new(qdrant_client), collection_name.clone(), vector_dim)?;

    // Initialize Qdrant collection (consider moving this logic)
    vector_db_instance.initialize_collection().await.map_err(|e| {
        eprintln!("Failed to initialize Qdrant collection '{}': {}", collection_name, e);
        format!("Qdrant initialization failed: {}", e)
    })?;

    let vector_db: Arc<dyn VectorRepository> = Arc::new(vector_db_instance);
    let reference_service: Arc<dyn ReferenceService> = Arc::new(ReferenceServiceImpl::new(embedder, vector_db.clone()));

    // --- Initial Indexing (Replace SimpleDocumentIndex) ---
    // TODO: Trigger indexing based on config.reference.sources
    //       This likely belongs in a dedicated initialization step or service method.
    //       The old SimpleDocumentIndex logic is removed/replaced.
    // Example: Trigger indexing (this needs proper implementation)
    println!("Triggering initial document indexing based on config...");
    if let Err(e) = reference_service.index_sources(&config.reference.sources).await {
        eprintln!("Error during initial indexing: {}", e);
        // Decide if this should be a fatal error
    }
    println!("Initial indexing process started/completed.");

    // Remove old document_index field and initialization
    // let document_index = initialize_document_index(...);
    // let handler = MyHandler { document_index, reference_service };
    let handler = MyHandler { reference_service }; // Pass only the service Arc

    let transport = (stdin(), stdout());

    println!("Starting MCP server...");
    let server_handle = handler.serve(transport).await?;

    let shutdown_reason = server_handle.waiting().await?;
    println!("mc-mcp server finished. Reason: {:?}", shutdown_reason);

    Ok(())
}

#[derive(Clone)]
struct MyHandler {
    // document_index: Arc<Mutex<SimpleDocumentIndex>>, // Removed
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
    use crate::application::reference_service::SearchResult;
    use crate::domain::reference::SearchQuery;
    use anyhow::Result;
    use async_trait::async_trait;
    use std::path::PathBuf;
    use crate::config::DocumentSource;

    // Keep MockReferenceService but update its methods if needed
    #[derive(Clone)] // Add Clone
    struct MockReferenceService;

    #[async_trait]
    impl ReferenceService for MockReferenceService {
        async fn index_documents(&self, _docs_path: Option<PathBuf>) -> Result<()> {
            println!("MockReferenceService: index_documents called (legacy)");
            Ok(())
        }
        // Add the new index_sources method
        async fn index_sources(&self, sources: &[DocumentSource]) -> Result<()> {
            println!("MockReferenceService: index_sources called with {} sources", sources.len());
            Ok(())
        }
        async fn search_documents(&self, query: SearchQuery, _score_threshold: Option<f32>) -> Result<Vec<SearchResult>> {
            println!("MockReferenceService: search_documents called with query: {:?}", query);
            if query.text == "found" {
                Ok(vec![SearchResult {
                    file_path: "mock/path.md".to_string(),
                    score: 0.9,
                    content_chunk: "mock content".to_string(),
                    metadata: None, // Add metadata later
                    source: Some("mock_source".to_string()), // Add source later
                }])
            } else {
                Ok(vec![])
            }
        }
    }

    // Create a helper to setup MyHandler with MockReferenceService
    fn setup_mock_handler() -> MyHandler {
        MyHandler {
            reference_service: Arc::new(MockReferenceService)
        }
    }

    #[tokio::test]
    async fn test_forge_test_execution() {
        // This test might fail if forge is not installed or project not set up
        // Consider mocking the Command execution or running this as an integration test
        // For now, assume it works or skip if forge isn't available
        if Command::new("forge").arg("--version").output().await.is_err() {
            println!("Skipping test_forge_test_execution: 'forge' command not found.");
            return;
        }
        let handler = setup_mock_handler(); // Use mock handler
        let result = handler.forge_test().await;
        // Basic check: Ensure it doesn't return the specific error message for command execution failure
        // A more robust test would mock the Command itself.
        assert!(!result.unwrap_err().starts_with("Failed to execute forge command"), "Forge command execution likely failed");
        // Or assert!(result.is_ok(), "forge test command failed"); // If expecting success
    }

    #[tokio::test]
    async fn test_call_tool_search_docs_semantic() {
        let handler = setup_mock_handler();
        let params = CallToolRequestParam {
            method: CallToolRequestMethod {},
            name: "search_docs".into(),
            arguments: Some(serde_json::json!({ "query": "found" }).as_object().unwrap().clone()),
            client_info: None, // Added None
            peer: None, // Added None
        };
        let result = handler.call_tool(params, RequestContext::new_for_test(AtomicU32RequestIdProvider::new(), ClientInfo::default())).await.unwrap(); // Use test context
        assert!(result.is_success());
        let content = result.success_content().unwrap();
        assert_eq!(content.len(), 1);
        assert!(content[0].text().unwrap().contains("mock/path.md"));
    }

    #[tokio::test]
    async fn test_call_tool_search_docs_no_results() {
         let handler = setup_mock_handler();
         let params = CallToolRequestParam {
             method: CallToolRequestMethod {},
             name: "search_docs".into(),
             arguments: Some(serde_json::json!({ "query": "notfound" }).as_object().unwrap().clone()),
             client_info: None,
             peer: None,
         };
         let result = handler.call_tool(params, RequestContext::new_for_test(AtomicU32RequestIdProvider::new(), ClientInfo::default())).await.unwrap();
         assert!(result.is_success()); // Search itself succeeds, just returns no results
         let content = result.success_content().unwrap();
         assert!(content[0].text().unwrap().contains("No documents found"));
     }

    #[tokio::test]
    async fn test_call_tool_search_docs_missing_query() {
        let handler = setup_mock_handler();
        let params = CallToolRequestParam {
            method: CallToolRequestMethod {},
            name: "search_docs".into(),
            arguments: Some(serde_json::Map::new()), // Empty arguments
            client_info: None,
            peer: None,
        };
        let result = handler.call_tool(params, RequestContext::new_for_test(AtomicU32RequestIdProvider::new(), ClientInfo::default())).await.unwrap();
        assert!(result.is_error());
        assert!(result.error_content().unwrap()[0].text().unwrap().contains("Missing 'query' parameter"));
    }

     #[tokio::test]
     async fn test_call_tool_search_docs_invalid_query() {
         let handler = setup_mock_handler();
         let params = CallToolRequestParam {
             method: CallToolRequestMethod {},
             name: "search_docs".into(),
             arguments: Some(serde_json::json!({ "query": 123 }).as_object().unwrap().clone()), // Non-string query
             client_info: None,
             peer: None,
         };
         let result = handler.call_tool(params, RequestContext::new_for_test(AtomicU32RequestIdProvider::new(), ClientInfo::default())).await.unwrap();
         assert!(result.is_error());
         assert!(result.error_content().unwrap()[0].text().unwrap().contains("Invalid 'query' parameter"));
     }

    #[tokio::test]
    async fn test_call_tool_unknown_tool() {
        let handler = setup_mock_handler();
        let params = CallToolRequestParam {
            method: CallToolRequestMethod {},
            name: "unknown_tool".into(),
            arguments: None,
            client_info: None,
            peer: None,
        };
        let result = handler.call_tool(params, RequestContext::new_for_test(AtomicU32RequestIdProvider::new(), ClientInfo::default())).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::MethodNotFound { .. }));
    }

    // Add tests for MyHandler::search_docs_semantic directly if needed
    // #[tokio::test]
    // async fn test_myhandler_search_semantic_found() {
    //     let handler = setup_mock_handler();
    //     let results = handler.search_docs_semantic("found", 5).await;
    //     assert_eq!(results.len(), 1);
    //     assert!(results[0].text().unwrap().contains("mock/path.md"));
    // }
    //
    // #[tokio::test]
    // async fn test_myhandler_search_semantic_not_found() {
    //     let handler = setup_mock_handler();
    //     let results = handler.search_docs_semantic("notfound", 5).await;
    //     assert_eq!(results.len(), 1);
    //     assert!(results[0].text().unwrap().contains("No documents found"));
    // }
}

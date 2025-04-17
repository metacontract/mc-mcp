// NOTE: use statements will need adjustment after refactoring
use rmcp::{
    model::{Content, ServerInfo, ServerCapabilities, Implementation, CallToolResult, ProtocolVersion, PaginatedRequestParam, ListResourcesResult, ReadResourceRequestParam, ReadResourceResult, ListPromptsResult, GetPromptRequestParam, GetPromptResult, ListResourceTemplatesResult},
    ServiceExt,
    ServerHandler,
    service::RequestContext,
    RoleServer,
    Error as McpError,
    tool,
    schemars::{self, JsonSchema},
};
use std::sync::Arc;
use rmcp::serde_json::{json};
use tokio::{
    io::{stdin, stdout},
    process::Command,
};
use serde::{Deserialize};
use serde_json::Value as JsonValue;

// Declare modules
mod domain;
mod config;
mod application;
mod infrastructure;

// Update use paths to crate::<module>::...
use crate::application::reference_service::ReferenceServiceImpl;
use crate::domain::reference::ReferenceService;
use crate::infrastructure::embedding::EmbeddingGenerator;
use crate::infrastructure::EmbeddingModel;
use crate::infrastructure::vector_db::{VectorDb, qdrant_client};
use crate::domain::vector_repository::VectorRepository;
use log;
use env_logger;
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    log::info!("mc-mcp server (MCP over stdio) started.");

    // --- Load Configuration ---
    let config = config::load_config()?;
    log::info!("Configuration loaded: {:?}", config);

    // --- Dependency Injection Setup ---
    let qdrant_url = std::env::var("QDRANT_URL").unwrap_or_else(|_| "http://localhost:6334".to_string());
    let qdrant_client = qdrant_client::Qdrant::from_url(&qdrant_url).build()?;
    let collection_name = std::env::var("QDRANT_COLLECTION").unwrap_or_else(|_| "mc_docs".to_string());
    let vector_dim: u64 = std::env::var("EMBEDDING_DIM").ok().and_then(|s| s.parse().ok()).unwrap_or(384);
    let embedding_model = EmbeddingModel::AllMiniLML6V2;
    let embedder = Arc::new(EmbeddingGenerator::new(embedding_model)?);
    let vector_db_instance = VectorDb::new(Box::new(qdrant_client), collection_name.clone(), vector_dim)?;
    vector_db_instance.initialize_collection().await?;
    let vector_db: Arc<dyn VectorRepository> = Arc::new(vector_db_instance);
    let reference_service: Arc<dyn ReferenceService> = Arc::new(ReferenceServiceImpl::new(embedder, vector_db.clone()));

    // --- Initial Indexing ---
    log::info!("Triggering initial document indexing based on config...");
    if let Err(e) = reference_service.index_sources(&config.reference.sources).await {
        log::error!("Error during initial indexing: {}", e);
    }
    log::info!("Initial indexing process started/completed." );

    let handler = MyHandler { reference_service };
    let transport = (stdin(), stdout());

    log::info!("Starting MCP server...");
    let server_handle = handler.serve(transport).await.inspect_err(|e| {
        log::error!("serving error: {:?}", e);
    })?;

    log::info!("mc-mcp server running, waiting for completion...");
    let shutdown_reason = server_handle.waiting().await?;
    log::info!("mc-mcp server finished. Reason: {:?}", shutdown_reason);

    Ok(())
}

#[derive(Clone)]
struct MyHandler {
    reference_service: Arc<dyn ReferenceService>,
}

const MAX_SEARCH_RESULTS: usize = 5;

#[derive(Debug, Deserialize, JsonSchema)]
struct SearchDocsArgs {
    #[schemars(description = "Natural language query for semantic search")]
    query: String,
    #[schemars(description = "Optional maximum number of results (default 5)")]
    limit: Option<usize>,
}

#[tool(tool_box)]
impl MyHandler {
    fn new(reference_service: Arc<dyn ReferenceService>) -> Self {
        Self { reference_service }
    }

    #[tool(description = "Run 'forge test' in the workspace.")]
    async fn forge_test(&self) -> Result<CallToolResult, McpError> {
        log::info!("Executing forge test tool...");
        let output_result = Command::new("forge")
            .arg("test")
            .output()
            .await;

        match output_result {
            Ok(output) => {
                log::info!("Forge test finished.");
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let status_code = output.status.code().map_or("N/A".to_string(), |c| c.to_string());

                let result_text = format!(
                    "Forge Test Results:\nExit Code: {}\n\nStdout:\n{}\nStderr:\n{}",
                    status_code,
                    stdout,
                    stderr
                );
                Ok(CallToolResult::success(vec![Content::text(result_text)]))
            }
            Err(e) => {
                log::error!("Failed to execute forge test: {}", e);
                let err_msg = format!("Failed to execute forge command: {}. Make sure 'forge' is installed and in PATH.", e);
                Ok(CallToolResult::error(vec![Content::text(err_msg)]))
            }
        }
    }

    #[tool(description = "Search metacontract documents semantically.")]
    async fn search_docs_semantic(&self, #[tool(aggr)] args: SearchDocsArgs) -> Result<CallToolResult, McpError> {
        let query = args.query;
        let limit = args.limit.unwrap_or(MAX_SEARCH_RESULTS);
        log::info!("Executing semantic search tool with query: '{}', limit: {}", query, limit);

        let search_query = crate::domain::reference::SearchQuery {
            text: query.clone(),
            limit: Some(limit),
        };
        match self.reference_service.search_documents(search_query, None).await {
            Ok(results) => {
                match serde_json::to_value(results) {
                    Ok(json_value) => {
                        log::debug!("Successfully serialized search results to JSON");
                        match Content::json(json_value) {
                            Ok(content) => Ok(CallToolResult::success(vec![content])),
                            Err(e) => {
                                log::error!("Failed to create JSON Content: {:?}", e);
                                let err_msg = format!("Failed to create JSON response: {:?}", e);
                                Ok(CallToolResult::error(vec![Content::text(err_msg)]))
                            }
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to serialize search results to JSON: {}", e);
                        let err_msg = format!("Failed to serialize search results: {}", e);
                        Ok(CallToolResult::error(vec![Content::text(err_msg)]))
                    }
                }
            }
            Err(e) => {
                log::error!("Semantic search failed: {}", e);
                let err_msg = format!("Semantic search failed: {}", e);
                Ok(CallToolResult::error(vec![Content::text(err_msg)]))
            }
        }
    }
}

#[tool(tool_box)]
impl ServerHandler for MyHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .build(),
            server_info: Implementation::from_build_env(),
            instructions: Some("This server can run forge tests and perform semantic search on indexed documents.".into()),
        }
    }

    async fn list_resources(
        &self,
        _request: PaginatedRequestParam,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult {
            resources: vec![],
            next_cursor: None,
        })
    }

    async fn read_resource(
        &self,
        ReadResourceRequestParam { uri }: ReadResourceRequestParam,
        _: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        Err(McpError::resource_not_found(
            "Resource feature not implemented",
            Some(json!({ "uri": uri })),
        ))
    }

    async fn list_prompts(
        &self,
        _request: PaginatedRequestParam,
        _: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        Ok(ListPromptsResult {
            next_cursor: None,
            prompts: vec![],
        })
    }

    async fn get_prompt(
        &self,
        GetPromptRequestParam { name, arguments: _ }: GetPromptRequestParam,
        _: RequestContext<RoleServer>,
    ) -> Result<GetPromptResult, McpError> {
        Err(McpError::invalid_params(format!("Prompt feature not implemented: {}", name), None))
    }

    async fn list_resource_templates(
        &self,
        _request: PaginatedRequestParam,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        Ok(ListResourceTemplatesResult {
            next_cursor: None,
            resource_templates: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use crate::domain::reference::SearchResult;
    use crate::config::DocumentSource;
    use anyhow::Result;
    use async_trait::async_trait;
    use downcast_rs::{DowncastSync, impl_downcast};

    use std::path::PathBuf;
    use rmcp::serde_json::{json};
    use rmcp::model::ContentPart;

    #[derive(Clone)]
    struct MockReferenceService {
        results_to_return: Arc<Mutex<Vec<SearchResult>>>,
        indexed_sources: Arc<Mutex<Vec<DocumentSource>>>,
    }

    impl Default for MockReferenceService {
        fn default() -> Self {
            Self {
                results_to_return: Arc::new(Mutex::new(Vec::new())),
                indexed_sources: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl MockReferenceService {
        fn set_search_results(&self, results: Vec<SearchResult>) {
            let mut lock = self.results_to_return.lock().unwrap();
            *lock = results;
        }
    }

    #[async_trait::async_trait]
    impl ReferenceService for MockReferenceService {
        async fn index_documents(&self, _docs_path: Option<PathBuf>) -> Result<()> {
            println!("MockReferenceService: index_documents called (dummy impl)");
            Ok(())
        }
        async fn index_sources(&self, sources: &[DocumentSource]) -> Result<()> {
            println!("MockReferenceService: index_sources called with {} sources", sources.len());
            self.indexed_sources.lock().unwrap().extend_from_slice(sources);
            Ok(())
        }
        async fn search_documents(&self, query: crate::domain::reference::SearchQuery, _score_threshold: Option<f32>) -> Result<Vec<SearchResult>> {
            println!("MockReferenceService: search_documents called with query: {:?}", query);
            let results = self.results_to_return.lock().unwrap().clone();
            Ok(results)
        }
    }

    impl_downcast!(sync MockReferenceService);

    fn setup_mock_handler() -> MyHandler {
        let mock_service: Arc<dyn ReferenceService + Send + Sync> = Arc::new(MockReferenceService::default());
        MyHandler::new(mock_service)
    }

    #[tokio::test]
    async fn test_search_docs_semantic_rich_output_tool() {
        let handler = setup_mock_handler();
        let mock_service = handler.reference_service.downcast_arc::<MockReferenceService>().expect("Should be MockReferenceService");
        mock_service.set_search_results(vec![SearchResult {
            file_path: "a.md".to_string(),
            score: 0.85,
            source: Some("SourceA".to_string()),
            content_chunk: "Chunk 1 content".to_string(),
            metadata: Some(json!({ "tag": "test" })),
        }]);

        let args = SearchDocsArgs { query: "test query".to_string(), limit: Some(3) };
        let result = handler.search_docs_semantic(args).await.expect("Tool call failed");

        assert!(result.is_success());
        let content_vec = result.into_success().unwrap();
        assert_eq!(content_vec.len(), 1);
        let content_result = content_vec[0].try_into_json().expect("Expected JSON content");
        let deserialized_results: Vec<SearchResult> = serde_json::from_value(content_result.json).expect("Failed to deserialize JSON");

        assert_eq!(deserialized_results.len(), 1);
        assert_eq!(deserialized_results[0].file_path, "a.md");
        assert_eq!(deserialized_results[0].score, 0.85);
        assert_eq!(deserialized_results[0].source, Some("SourceA"));
        assert_eq!(deserialized_results[0].content_chunk, "Chunk 1 content");
        assert_eq!(deserialized_results[0].metadata, Some(json!({ "tag": "test" })));
    }

    #[tokio::test]
    async fn test_search_docs_semantic_no_results_output_tool() {
        let handler = setup_mock_handler();
        let mock_service = handler.reference_service.downcast_arc::<MockReferenceService>().expect("Should be MockReferenceService");
        mock_service.set_search_results(vec![]);

        let args = SearchDocsArgs { query: "nonexistent".to_string(), limit: Some(3) };
        let result = handler.search_docs_semantic(args).await.expect("Tool call failed");

        assert!(result.is_success());
        let content_vec = result.into_success().unwrap();
        assert_eq!(content_vec.len(), 1);
        let content_result = content_vec[0].try_into_json().expect("Expected JSON content");
        let deserialized_results: Vec<SearchResult> = serde_json::from_value(content_result.json).expect("Failed to deserialize JSON");

        assert!(deserialized_results.is_empty());
    }

     #[tokio::test]
    async fn test_forge_test_tool_success_mocked() {
        let handler = setup_mock_handler();

        let result = handler.forge_test().await.expect("Tool call failed");

        assert!(result.is_error());
        let content_vec = result.into_error().unwrap();
        assert_eq!(content_vec.len(), 1);
        let error_text = content_vec[0].try_into_text().expect("Expected text content");
        assert!(error_text.text.contains("Failed to execute forge command"));
    }

    /*
    use rmcp::transport::stdio;
    use std::time::Duration;

     #[tokio::test]
    async fn test_search_endpoint_success() {
        // ... test remains commented ...
        // Note: The test setup would need significant changes
        //       to use the #[tool] approach, likely involving
        //       a mock client or a different testing strategy.
    }

     #[tokio::test]
    async fn test_search_endpoint_no_results() {
        // ... test remains commented ...
    }
    */
}

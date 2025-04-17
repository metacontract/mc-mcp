// NOTE: use statements will need adjustment after refactoring
use rmcp::{
    model::{Content, ServerInfo, ServerCapabilities, Implementation, CallToolResult, ProtocolVersion, PaginatedRequestParam, ListResourcesResult, ReadResourceRequestParam, ReadResourceResult, ListPromptsResult, GetPromptRequestParam, GetPromptResult, ListResourceTemplatesResult, RawContent},
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

// Import from the library crate using its name (mc-mcp)
use mc_mcp::application::reference_service::ReferenceServiceImpl;
use mc_mcp::domain::reference::ReferenceService;
use mc_mcp::infrastructure::embedding::EmbeddingGenerator;
use mc_mcp::infrastructure::EmbeddingModel;
use mc_mcp::infrastructure::vector_db::{VectorDb, qdrant_client};
use mc_mcp::domain::vector_repository::VectorRepository;
use mc_mcp::config; // Import config module directly
use mc_mcp::file_system; // Import file_system module directly
use mc_mcp::domain; // Import domain for SearchQuery
use mc_mcp::config::DocumentSource; // Import DocumentSource for MockReferenceService

use log;
use env_logger;
use anyhow::Result;
// Add imports for Qdrant types used in mock impl
use qdrant_client::qdrant::{ScoredPoint, PointStruct};
// Import BoxFuture for mock impl signature
use futures::future::BoxFuture;

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    log::info!("mc-mcp server (MCP over stdio) started.");

    // --- Load Configuration ---
    let config = config::load_config()?;
    log::info!("Configuration loaded: {:?}", config);

    // --- Dependency Injection Setup (Moved earlier to access vector_db) ---
    let qdrant_url = std::env::var("QDRANT_URL").unwrap_or_else(|_| "http://localhost:6334".to_string());
    let qdrant_client = qdrant_client::Qdrant::from_url(&qdrant_url).build()?;
    let collection_name = std::env::var("QDRANT_COLLECTION").unwrap_or_else(|_| "mc_docs".to_string());
    let vector_dim: u64 = std::env::var("EMBEDDING_DIM").ok().and_then(|s| s.parse().ok()).unwrap_or(384);
    let embedding_model = EmbeddingModel::AllMiniLML6V2;
    let embedder = Arc::new(EmbeddingGenerator::new(embedding_model)?);
    let vector_db_instance = VectorDb::new(Box::new(qdrant_client), collection_name.clone(), vector_dim)?;
    // Initialize collection *before* potentially upserting prebuilt index
    vector_db_instance.initialize_collection().await?;
    // Get Arc<dyn VectorRepository> to use for both prebuilt index and service
    let vector_db: Arc<dyn VectorRepository> = Arc::new(vector_db_instance);
    // Keep reference_service initialization separate for now
    // let reference_service: Arc<dyn ReferenceService> = Arc::new(ReferenceServiceImpl::new(embedder.clone(), vector_db.clone()));

    // --- Prebuilt Index Loading (If configured) ---
    if let Some(prebuilt_path) = &config.reference.prebuilt_index_path {
        log::info!("Attempting to load prebuilt index from: {:?}", prebuilt_path);
        match file_system::load_prebuilt_index(prebuilt_path.clone()) {
            Ok(prebuilt_docs) => {
                log::info!("Successfully loaded {} documents from prebuilt index.", prebuilt_docs.len());
                if !prebuilt_docs.is_empty() {
                    log::info!("Upserting prebuilt documents...");
                    // Use the vector_db Arc directly to upsert
                    match vector_db.upsert_documents(&prebuilt_docs).await {
                        Ok(_) => log::info!("Successfully upserted prebuilt documents."),
                        Err(e) => log::error!("Failed to upsert prebuilt documents: {}", e),
                    }
                }
            }
            Err(e) => {
                // Log the error but continue without the prebuilt index
                log::error!("Failed to load or parse prebuilt index from {:?}: {}. Continuing without prebuilt index.", prebuilt_path, e);
            }
        }
    } else {
        log::info!("No prebuilt index path configured.");
    }

    // --- Initialize Reference Service (after potential prebuilt index loading) ---
    let reference_service: Arc<dyn ReferenceService> = Arc::new(ReferenceServiceImpl::new(embedder, vector_db.clone()));

    // --- Initial Indexing from configured sources (after potential prebuilt index loading) ---
    log::info!("Triggering indexing from configured sources ({} sources)...", config.reference.sources.len());
    // Decide if we should always index sources, or skip if prebuilt was loaded?
    // For now, let's always index configured sources after loading prebuilt.
    // Duplicates might be overwritten depending on VectorDb implementation.
    if let Err(e) = reference_service.index_sources(&config.reference.sources).await {
        log::error!("Error during configured source indexing: {}", e);
    }
    log::info!("Configured source indexing process started/completed." );

    // --- Start MCP Server ---
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
                log::info!("Forge test finished with status: {:?}", output.status);
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let status_code = output.status.code().map_or("N/A".to_string(), |c| c.to_string());

                let result_text = format!(
                    "Forge Test Results:\nExit Code: {}\n\nStdout:\n{}\nStderr:\n{}",
                    status_code,
                    stdout,
                    stderr
                );

                // Check the exit status of the command
                if output.status.success() {
                    log::info!("Forge test reported success.");
                    Ok(CallToolResult::success(vec![Content::text(result_text)]))
                } else {
                    log::warn!("Forge test reported failure (non-zero exit code).");
                    // Return error result if the command executed but failed
                    Ok(CallToolResult::error(vec![Content::text(result_text)]))
                }
            }
            Err(e) => { // If the 'forge' command itself fails to run
                log::error!("Failed to execute forge test command: {}", e);
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

        let search_query = domain::reference::SearchQuery {
            text: query.clone(),
            limit: Some(limit),
        };
        match self.reference_service.search_documents(search_query, None).await {
            Ok(results) => {
                match serde_json::to_value(results) {
                    Ok(json_value) => {
                        log::debug!("Successfully serialized search results to JSON");
                        match serde_json::to_string(&json_value) {
                            Ok(json_string) => Ok(CallToolResult::success(vec![Content::text(json_string)])),
                            Err(e) => {
                                log::error!("Failed to serialize JSON to string: {:?}", e);
                                let err_msg = format!("Failed to create text response from JSON: {:?}", e);
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
    // Use the library crate path for domain items in tests
    use mc_mcp::domain::reference::SearchResult;
    use mc_mcp::config::DocumentSource; // Import DocumentSource via library crate
    use mc_mcp::ReferenceService; // Import trait via library crate
    use anyhow::Result;
    use async_trait::async_trait;
    use rmcp::model::Content;

    use rmcp::serde_json::{self, json};

    // --- Mock ReferenceService --- //
    #[derive(Clone, Default)] // Add Default derive
    struct MockReferenceService {
        results_to_return: Arc<Mutex<Vec<SearchResult>>>,
        indexed_sources: Arc<Mutex<Vec<DocumentSource>>>,
    }

    impl MockReferenceService {
        fn set_search_results(&self, results: Vec<SearchResult>) {
            let mut lock = self.results_to_return.lock().unwrap();
            *lock = results;
        }
        fn get_indexed_sources(&self) -> Vec<DocumentSource> {
             self.indexed_sources.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl ReferenceService for MockReferenceService {
        // index_sources implementation
        async fn index_sources(&self, sources: &[DocumentSource]) -> Result<()> {
            println!("MockReferenceService: index_sources called with {} sources", sources.len());
            self.indexed_sources.lock().unwrap().extend_from_slice(sources);
            Ok(())
        }
        // search_documents implementation
        async fn search_documents(&self, query: mc_mcp::domain::reference::SearchQuery, _score_threshold: Option<f32>) -> Result<Vec<SearchResult>> {
            println!("MockReferenceService: search_documents called with query: {:?}", query);
            let results = self.results_to_return.lock().unwrap().clone();
            Ok(results)
        }
         // Dummy implementations for non-async trait methods (required by compiler)
        fn search(&self, _collection_name: String, _vector: Vec<f32>, _limit: u64) -> BoxFuture<Result<Vec<ScoredPoint>, String>> {
            unimplemented!("Mock search not needed for these tests")
        }
        fn upsert(&self, _collection_name: String, _points: Vec<PointStruct>) -> BoxFuture<Result<(), String>> {
             unimplemented!("Mock upsert not needed for these tests")
        }
    }

    // --- Test Setup --- //
    // Helper function returns concrete mock type now
    fn setup_mock_handler() -> (MyHandler, Arc<MockReferenceService>) {
        let mock_service = Arc::new(MockReferenceService::default());
        let handler = MyHandler { reference_service: mock_service.clone() };
        (handler, mock_service)
    }

    // --- Tests --- //
    #[tokio::test]
    async fn test_search_docs_semantic_rich_output_tool() {
        let (handler, mock_service) = setup_mock_handler(); // Get concrete mock
        let mock_result = SearchResult {
            file_path: "a.md".to_string(),
            score: 0.85,
            source: Some("SourceA".to_string()),
            content_chunk: "Chunk 1 content".to_string(),
            metadata: Some(json!({ "tag": "test" })),
        };
        mock_service.set_search_results(vec![mock_result.clone()]);

        let args = SearchDocsArgs { query: "test query".to_string(), limit: Some(3) };
        let result = handler.search_docs_semantic(args).await.expect("Tool call failed");

        assert_eq!(result.is_error, Some(false));
        assert_eq!(result.content.len(), 1);

        // Match on annotated_content.raw
        let annotated_content = &result.content[0];
        match &annotated_content.raw { // Match on .raw field
            RawContent::Text(text) => { // Expect Text containing JSON string
                // Parse the JSON string from the text content
                let parsed_results: Vec<SearchResult> = serde_json::from_str(&text.text)
                    .expect("Failed to parse SearchResult from text content");
                assert_eq!(parsed_results.len(), 1, "Expected 1 search result");
                // Compare with the original mock_result
                assert_eq!(parsed_results[0].file_path, mock_result.file_path);
                assert_eq!(parsed_results[0].score, mock_result.score);
                assert_eq!(parsed_results[0].source, mock_result.source);
                assert_eq!(parsed_results[0].content_chunk, mock_result.content_chunk);
                assert_eq!(parsed_results[0].metadata, mock_result.metadata);
            }
            /* // Comment out Json variant as it's not found in rmcp v0.1.5
            RawContent::Json { json } => {
                // Handle JSON content - assuming search results are in a specific structure
                let results: Vec<SearchResult> = serde_json::from_value(json.clone())?
                    .expect("Failed to parse search results from JSON");
                assert_eq!(results.len(), 1, "Expected 1 search result");
                assert_eq!(results[0].document_source.uri, "file:///path/to/doc1.md");
            }
            */
            // RawContent::Text handled above, panic for other unexpected types
            other_kind => panic!("Expected Text content containing JSON, found {:?}", other_kind),
        }
    }

    #[tokio::test]
    async fn test_search_docs_semantic_no_results_output_tool() {
        let (handler, mock_service) = setup_mock_handler();
        mock_service.set_search_results(vec![]); // No results

        let args = SearchDocsArgs { query: "nonexistent".to_string(), limit: Some(3) };
        let result = handler.search_docs_semantic(args).await.expect("Tool call failed");

        assert_eq!(result.is_error, Some(false));
        assert_eq!(result.content.len(), 1);

        // Match on annotated_content.raw
        let annotated_content = &result.content[0];
        match &annotated_content.raw { // Match on .raw field
            RawContent::Text(text) => { // Expect Text containing JSON string
                // Parse the JSON string (expecting empty array)
                let parsed_results: Vec<SearchResult> = serde_json::from_str(&text.text)
                    .expect("Failed to parse SearchResult from text content");
                assert!(parsed_results.is_empty(), "Expected empty results array");
            }
            /* // Comment out Json variant as it's not found in rmcp v0.1.5
            RawContent::Json { json } => { // Match on RawContent::Json variant
                // Expecting empty JSON array `[]` or similar structure for no results
                let deserialized_results: Vec<SearchResult> = serde_json::from_value(json.clone()).expect("Failed to deserialize JSON");
                assert!(deserialized_results.is_empty(), "Expected empty results array");
            }
            */
            // RawContent::Text handled above, panic for other unexpected types
            other_kind => panic!("Expected Text content containing JSON, found {:?}", other_kind),
        }
    }

    #[tokio::test]
    #[ignore] // Ignore this test for now, requires forge setup that succeeds
    async fn test_forge_test_tool_runs_successfully() {
        let (handler, _mock_service) = setup_mock_handler();

        // Assume forge command exists and runs without internal errors for this test
        let result = handler.forge_test().await.expect("Tool call should not panic");

        // Expect success (is_error = false) when forge command runs
        assert_eq!(result.is_error, Some(false), "Expected is_error to be false when forge runs");
        assert!(!result.content.is_empty(), "Expected content to be non-empty");

        // Optional: Further check content if needed, e.g., look for typical success output
        let annotated_content = &result.content[0];
        let _ = match &annotated_content.raw { // Match on .raw field and bind to _
            RawContent::Text(text) => {
                // We can't reliably assert on the exact output, but we know it should be text
                assert!(!text.text.is_empty(), "Expected non-empty text output");
            },
            other_kind => panic!("Expected Text content, found {:?}", other_kind),
        };
    }

    #[tokio::test]
    // This test should pass in environments where `forge test` runs but fails internally
    async fn test_forge_test_tool_reports_failure() {
        let (handler, _mock_service) = setup_mock_handler();

        // Assume forge command exists but fails internally (non-zero exit code)
        let result = handler.forge_test().await.expect("Tool call should not panic");

        // Expect error (is_error = true) when forge command fails internally
        assert_eq!(result.is_error, Some(true), "Expected is_error to be true when forge fails");
        assert!(!result.content.is_empty(), "Expected content to be non-empty");

        // Optional: Check for specific failure messages in content if needed
        let annotated_content = &result.content[0];
        let _ = match &annotated_content.raw { // Match on .raw field and bind to _
            RawContent::Text(text) => {
                assert!(!text.text.is_empty(), "Expected non-empty text output for failure");
            },
            other_kind => panic!("Expected Text content for failure, found {:?}", other_kind),
        };
    }
}


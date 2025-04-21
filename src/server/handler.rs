use rmcp::serde_json::json;
use rmcp::{
    model::{
        CallToolResult, Content, GetPromptRequestParam, GetPromptResult, Implementation,
        ListPromptsResult, ListResourceTemplatesResult, ListResourcesResult, PaginatedRequestParam,
        ProtocolVersion, ReadResourceRequestParam, ReadResourceResult, ServerCapabilities,
        ServerInfo,
    },
    schemars::{self, JsonSchema},
    service::RequestContext,
    tool, Error as McpError, RoleServer, ServerHandler,
};
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use tokio::process::Command;
use log;


// Import from the library crate using its name (mc-mcp)
use crate::config::McpConfig;
use crate::domain; // Import domain for SearchQuery
use crate::domain::reference::ReferenceService;


/// Handler for the MCP server logic.
#[derive(Clone)]
pub struct McpServerHandler {
    // Use Mutex<Option<...>> to hold the service, allowing it to be initialized later
    pub reference_service_state: Arc<Mutex<Option<Arc<dyn ReferenceService>>>>,
    pub config: Arc<McpConfig>,
}

const MAX_SEARCH_RESULTS: usize = 5;

#[derive(Debug, Deserialize, JsonSchema)]
struct SearchDocsArgs {
    #[schemars(description = "Natural language query for semantic search")]
    query: String,
    #[schemars(description = "Optional maximum number of results (default 5)")]
    limit: Option<usize>,
}

/// Arguments for the `mc_deploy` tool.
// Define args for mc_deploy tool
// Ensure JsonSchema is derived
#[derive(Debug, Deserialize, JsonSchema)]
struct McDeployArgs {
    #[schemars(description = "Whether to broadcast the transaction (execute actual deployment) or perform a dry run")]
    broadcast: Option<bool>,
}

/// Arguments for the `mc_upgrade` tool.
// Define args for mc_upgrade tool
#[derive(Debug, Deserialize, JsonSchema)]
struct McUpgradeArgs {
    #[schemars(description = "Whether to broadcast the transaction (execute actual upgrade) or perform a dry run")]
    broadcast: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct Erc7201SlotArgs {
    #[schemars(description = "ERC7201 namespace.slot (e.g. NFT.ERC721State)")]
    slot: String,
}

#[tool(tool_box)]
impl McpServerHandler {
    /// Creates a new handler instance with uninitialized service state.
    pub fn new(config: Arc<McpConfig>) -> Self {
        Self {
            // Initialize service as None initially
            reference_service_state: Arc::new(Mutex::new(None)),
            config,
        }
    }

    /// Helper function to run forge script commands (deploy/upgrade).
    /// Takes the script type ("deploy" or "upgrade") and broadcast flag.
    /// Reads the script path from config, constructs and runs the `forge script` command,
    /// and returns the result as a `CallToolResult`.
    async fn run_forge_script(&self, script_type: &str, broadcast: bool) -> Result<CallToolResult, McpError> {
        // --- Get script path from config based on type ---
        let script_path = match script_type {
            "deploy" => self.config.scripts.deploy.as_deref(),
            "upgrade" => self.config.scripts.upgrade.as_deref(),
            _ => {
                log::error!("Invalid script type provided to helper: {}", script_type);
                return Err(McpError::internal_error("Invalid script type", None)); // Internal error
            }
        };

        let script_path = match script_path {
            Some(path) if !path.is_empty() => path.to_string(),
            _ => {
                // Capitalize only the first letter of script_type
                let type_capitalized = script_type.chars().next().map(|c| c.to_uppercase().collect::<String>() + &script_type[1..]).unwrap_or_else(|| script_type.to_string());
                let error_msg = format!("{} script path is not configured in mcp_config.toml ([scripts].{})", type_capitalized, script_type);
                log::error!("{}", error_msg);
                return Ok(CallToolResult::error(vec![Content::text(error_msg)]));
            }
        };

        log::info!(
            "Executing {}: script='{}', broadcast={}",
            script_type,
            script_path,
            broadcast
        );

        // --- Construct forge command ---
        let mut command = Command::new("forge");
        command.arg("script").arg(&script_path);
        if broadcast {
            command.arg("--broadcast");

            // --- Add broadcast-specific arguments ---
            let rpc_url = self.config.scripts.rpc_url.as_deref();
            let private_key_env = self.config.scripts.private_key_env_var.as_deref();

            match (rpc_url, private_key_env) {
                (Some(url), Some(env_var)) => {
                    if !url.is_empty() {
                        command.arg("--rpc-url").arg(url);
                    } else {
                        log::error!("Broadcast requested but RPC URL is empty in config.");
                        return Ok(CallToolResult::error(vec![Content::text(
                            "Broadcast requires a non-empty RPC URL configured in mcp_config.toml ([scripts].rpc_url).",
                        )]));
                    }
                    if !env_var.is_empty() {
                        // Pass the ENV VAR NAME to forge, forge will read the key from the env var itself.
                        command.arg("--private-key").arg(format!("${}", env_var));
                        log::info!("Using RPC URL: {} and private key from env var: {}", url, env_var);
                    } else {
                        log::error!("Broadcast requested but private key env var name is empty in config.");
                        return Ok(CallToolResult::error(vec![Content::text(
                            "Broadcast requires a non-empty private key environment variable name configured in mcp_config.toml ([scripts].private_key_env_var).",
                        )]));
                    }
                }
                (None, _) => {
                    log::error!("Broadcast requested but RPC URL is not configured.");
                    return Ok(CallToolResult::error(vec![Content::text(
                        "Broadcast requires an RPC URL configured in mcp_config.toml ([scripts].rpc_url).",
                    )]));
                }
                (_, None) => {
                    log::error!("Broadcast requested but private key env var name is not configured.");
                    return Ok(CallToolResult::error(vec![Content::text(
                        "Broadcast requires a private key environment variable name configured in mcp_config.toml ([scripts].private_key_env_var).",
                    )]));
                }
            }
            // TODO: Add other necessary broadcast args like --verifier? Maybe from config too?

        } else {
            log::info!("Dry run mode enabled.");
        }

        // --- Execute command ---
        log::debug!("Running command: {:?}", command);
        let output_result = command.output().await;

        match output_result {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                log::info!(
                    "forge script finished. Status: {:?}, stdout len: {}, stderr len: {}",
                    output.status.code(),
                    stdout.len(),
                    stderr.len()
                );
                log::debug!("forge stdout:
{}", stdout);
                log::debug!("forge stderr:
{}", stderr);

                let status_code = output
                    .status
                    .code()
                    .map_or("N/A".to_string(), |c| c.to_string());

                // Determine title based on script type and broadcast status
                let result_title = match (script_type, broadcast) {
                    ("deploy", true) => "Forge Deploy Results",
                    ("deploy", false) => "Forge Deploy Dry Run Results",
                    ("upgrade", true) => "Forge Upgrade Results",
                    ("upgrade", false) => "Forge Upgrade Dry Run Results",
                    _ => "Forge Script Results", // Fallback
                };

                let result_text = format!(
                    "{}:
Script: {}
Broadcast: {}
Exit Code: {}

Stdout:
{}
Stderr:
{}",
                    result_title,
                    script_path,
                    broadcast,
                    status_code,
                    stdout,
                    stderr
                );

                if output.status.success() {
                    log::info!("Forge script reported success.");
                    Ok(CallToolResult::success(vec![Content::text(result_text)]))
                } else {
                    log::warn!("Forge script reported failure.");
                    Ok(CallToolResult::error(vec![Content::text(result_text)]))
                }
            }
            Err(e) => {
                log::error!("Failed to execute forge script command: {}", e);
                let err_msg = format!("Failed to execute forge script command for '{}': {}. Make sure 'forge' is installed and in PATH.", script_path, e);
                Ok(CallToolResult::error(vec![Content::text(err_msg)]))
            }
        }
    }

    /// Runs `forge test` in the current workspace and returns the output.
    #[tool(description = "Run 'forge test' in the workspace.")]
    async fn mc_test(&self) -> Result<CallToolResult, McpError> {
        log::info!("Executing mc_test tool...");
        let output_result = Command::new("forge").arg("test").output().await;

        match output_result {
            Ok(output) => {
                log::info!("Forge test finished with status: {:?}", output.status);
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let status_code = output
                    .status
                    .code()
                    .map_or("N/A".to_string(), |c| c.to_string());

                let result_text = format!(
                    "Forge Test Results:
Exit Code: {}

Stdout:
{}
Stderr:
{}",
                    status_code, stdout, stderr
                );

                if output.status.success() {
                    log::info!("Forge test reported success.");
                    Ok(CallToolResult::success(vec![Content::text(result_text)]))
                } else {
                    log::warn!("Forge test reported failure (non-zero exit code).");
                    Ok(CallToolResult::error(vec![Content::text(result_text)]))
                }
            }
            Err(e) => {
                log::error!("Failed to execute forge test command: {}", e);
                let err_msg = format!("Failed to execute forge command: {}. Make sure 'forge' is installed and in PATH.", e);
                Ok(CallToolResult::error(vec![Content::text(err_msg)]))
            }
        }
    }

    /// Performs semantic search over configured documentation sources.
    /// Takes a natural language query and an optional limit.
    /// Returns a JSON string containing a list of search results (`Vec<SearchResult>`).
    #[tool(description = "Semantic search over metacontract documents.")]
    async fn mc_search_docs_semantic(
        &self,
        #[tool(aggr)] args: SearchDocsArgs,
    ) -> Result<CallToolResult, McpError> {
        let query = args.query;
        let limit = args.limit.unwrap_or(MAX_SEARCH_RESULTS);
        log::info!(
            "Executing mc_search_docs tool with query: '{}', limit: {}",
            query,
            limit
        );

        // --- Check if ReferenceService is initialized ---
        let service_maybe;
        { // Lock scope
            let state = self.reference_service_state.lock().unwrap();
            service_maybe = state.clone(); // Clone the Arc<dyn ReferenceService> if Some
        }

        if let Some(service) = service_maybe {
            // --- Service is available, proceed with search ---
            log::debug!("ReferenceService available, performing search.");
            let search_query = domain::reference::SearchQuery {
                text: query.clone(),
                limit: Some(limit),
                sources: None,
            };
            match service // Use the cloned Arc
                .search_documents(search_query, None)
                .await
            {
                Ok(results) => match serde_json::to_value(results) {
                    Ok(json_value) => {
                        log::debug!("Successfully serialized search results to JSON");
                        match serde_json::to_string(&json_value) {
                            Ok(json_string) => {
                                Ok(CallToolResult::success(vec![Content::text(json_string)]))
                            }
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
                },
                Err(e) => {
                    log::error!("Semantic search failed: {}", e);
                    let err_msg = format!("Semantic search failed: {}", e);
                    Ok(CallToolResult::error(vec![Content::text(err_msg)]))
                }
            }
        } else {
            // --- Service not yet initialized ---
            log::warn!("mc_search_docs_semantic called but ReferenceService is not yet initialized.");
            Ok(CallToolResult::error(vec![Content::text(
                "The document search service is still initializing. Please try again shortly.",
            )]))
        }
    }

    /// Initializes a new Foundry project using the metacontract template.
    /// This tool only works if the current directory is empty.
    /// It runs `forge init . -t metacontract/template`.
    #[tool(
        description = "Initialize a new Foundry project using a template. Only works in an empty directory unless [setup].force = true in config."
    )]
    pub async fn mc_setup(&self) -> Result<CallToolResult, McpError> {
        use std::env;
        use std::fs;
        use std::process::Command; // Use std::process::Command
        use tokio::task; // Import spawn_blocking

        // 1. Get MC_PROJECT_ROOT and move to it
        let project_root = std::env::var("MC_PROJECT_ROOT")
            .map_err(|_| McpError::internal_error("MC_PROJECT_ROOT is not set. Please set MC_PROJECT_ROOT to your project root directory.", None))?;
        let project_root_path = std::path::Path::new(&project_root);
        if !project_root_path.exists() {
            return Ok(CallToolResult::error(vec![Content::text(format!(
                "MC_PROJECT_ROOT does not exist: {}", project_root
            ))]));
        }
        // Note: Changing CWD globally can be problematic in async contexts.
        // Consider executing forge init with a specific working directory if possible.
        // For now, keep the global CWD change but be aware of potential issues.
        std::env::set_current_dir(&project_root_path)
            .map_err(|e| McpError::internal_error(format!("Failed to set current dir: {e}"), None))?;
        log::info!("Changed working directory to MC_PROJECT_ROOT: {:?}", project_root_path);
        // 2. Check if current directory is empty
        log::info!("Current working directory for mc_setup: {:?}", env::current_dir());
        let current_dir = env::current_dir().map_err(|e| {
            McpError::internal_error(format!("Failed to get current dir: {e}"), None)
        })?;
        let entries = fs::read_dir(&current_dir).map_err(|e| {
            McpError::internal_error(format!("Failed to read current dir: {e}"), None)
        })?;
        let is_empty = entries.into_iter().next().is_none();
        let force = self.config.setup.force;
        if !force && !is_empty {
            // If not forcing and directory is not empty, return error
            return Ok(CallToolResult::error(vec![Content::text(
                "The current directory is not empty. Please run setup in a new directory or set [setup].force = true in mcp_config.toml.",
            )]));
        }
        if force && !is_empty {
            log::warn!("[setup.force=true] Forcing setup: existing files in the directory may be overwritten.");
        }
        // 3. Run forge init . -t <repo>
        // Use local template cache if specified
        let template_arg = if let Ok(local_template) = std::env::var("MC_TEMPLATE_CACHE") {
            local_template
        } else {
            crate::MC_TEMPLATE_REPO.to_string() // Use crate::MC_TEMPLATE_REPO
        };

        // --- Execute forge init in a blocking task --- //
        let force_clone = force; // Clone force for the closure
        let template_arg_clone = template_arg.clone(); // Clone template_arg for the closure
        let output_result = task::spawn_blocking(move || {
            let mut cmd = Command::new("forge");
            cmd.args(["init", ".", "-t", &template_arg_clone]); // Use cloned template_arg
            if force_clone { // Use cloned force
                cmd.arg("--no-git");
            }
            log::info!("Running forge init command in blocking task: {:?}", cmd);
            cmd.output() // This is std::process::Command::output()
        }).await.map_err(|e| McpError::internal_error(format!("Spawn blocking task failed: {}", e), None))?;
        // --- End blocking task execution --- //

        match output_result { // Match the result from spawn_blocking
            Ok(output) => { // The inner result is Ok(std::process::Output)
                if output.status.success() {
                    Ok(CallToolResult::success(vec![Content::text(format!(
                        "Successfully initialized Foundry project with template: {}", template_arg
                    ))]))
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                    let err_msg = format!(
                        "forge init failed with exit code: {:?}.\nStderr:\n{}",
                        output.status.code(),
                        stderr
                    );
                    log::error!("mc_setup forge init failed: {}", err_msg);
                    Ok(CallToolResult::error(vec![Content::text(err_msg)]))
                }
            }
            Err(e) => { // The inner result is Err(std::io::Error)
                let err_msg = format!("Failed to execute forge init command: {}. Make sure 'forge' is installed and in PATH.", e);
                log::error!("mc_setup failed to execute forge: {}", e);
                Ok(CallToolResult::error(vec![Content::text(err_msg)]))
            }
        }
    }

    /// Deploys contracts using the script specified in `mcp_config.toml` -> `[scripts].deploy`.
    /// Supports dry-run (default) or actual broadcast (`broadcast: true`).
    #[tool(description = "Deploy contracts using a Foundry script.")]
    async fn mc_deploy(
        &self,
        #[tool(aggr)] args: McDeployArgs,
    ) -> Result<CallToolResult, McpError> {
        let broadcast = args.broadcast.unwrap_or(false);
        self.run_forge_script("deploy", broadcast).await
    }

    /// Upgrades contracts using the script specified in `mcp_config.toml` -> `[scripts].upgrade`.
    /// Supports dry-run (default) or actual broadcast (`broadcast: true`).
    #[tool(description = "Upgrade contracts using a Foundry script.")]
    async fn mc_upgrade(&self, #[tool(aggr)] args: McUpgradeArgs) -> Result<CallToolResult, McpError> {
        let broadcast = args.broadcast.unwrap_or(false);
        self.run_forge_script("upgrade", broadcast).await
    }

    /// Calculates ERC7201 storage slot using Foundry's cast index-erc7201.
    #[tool(description = "Calculate ERC7201 storage slot using Foundry's cast index-erc7201.")]
    async fn mc_erc7201_slot(
        &self,
        #[tool(aggr)] args: Erc7201SlotArgs,
    ) -> Result<CallToolResult, McpError> {
        let slot = args.slot;
        let output_result = tokio::process::Command::new("cast")
            .args(["index-erc7201", &slot])
            .output()
            .await;

        match output_result {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                if output.status.success() {
                    // 0xで始まり長さが66文字（0x+64バイト）でなければエラー扱い
                    if stdout.starts_with("0x") && stdout.len() == 66 {
                        Ok(CallToolResult::success(vec![Content::text(stdout)]))
                    } else {
                        Ok(CallToolResult::error(vec![Content::text(format!(
                            "cast index-erc7201 returned invalid slot: '{}'.\nStderr:\n{}",
                            stdout, stderr
                        ))]))
                    }
                } else {
                    Ok(CallToolResult::error(vec![Content::text(format!(
                        "cast index-erc7201 failed: {:?}\n{}",
                        output.status.code(), stderr
                    ))]))
                }
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Failed to execute cast: {}",
                e
            ))])),
        }
    }
}

#[tool(tool_box)]
impl ServerHandler for McpServerHandler {
    fn get_info(&self) -> ServerInfo {
        log::trace!("Entering get_info method..."); // <-- Add trace at the beginning of get_info
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                "This server can run forge tests and perform semantic search on indexed documents."
                    .into(),
            ),
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
        Err(McpError::invalid_params(
            format!("Prompt feature not implemented: {}", name),
            None,
        ))
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
    use crate::{config::McpConfig, domain::reference::SearchResult, ReferenceService}; // Use crate level imports
    use rmcp::model::RawContent; // Import RawContent only
    use rmcp::serde_json::{self, json};
    use std::sync::{Arc, Mutex};
    use futures::future::BoxFuture;
    use qdrant_client::qdrant::{PointStruct, ScoredPoint};
    use crate::config::DocumentSource; // Use crate::


    // --- Mock ReferenceService (Copied from main.rs test, adjust if necessary) --- //
    #[derive(Clone, Default)]
    struct MockReferenceService {
        results_to_return: Arc<Mutex<Vec<SearchResult>>>,
        indexed_sources: Arc<Mutex<Vec<DocumentSource>>>,
    }

    impl MockReferenceService {
        fn set_search_results(&self, results: Vec<SearchResult>) {
            let mut lock = self.results_to_return.lock().unwrap();
            *lock = results;
        }
        // Removed get_indexed_sources as it's not used in unit tests here
    }

    #[async_trait::async_trait]
    impl ReferenceService for MockReferenceService {
         async fn index_sources(&self, sources: &[DocumentSource]) -> anyhow::Result<()> {
            log::info!(
                "MockReferenceService: index_sources called with {} sources",
                sources.len()
            );
            self.indexed_sources
                .lock()
                .unwrap()
                .extend_from_slice(sources);
            Ok(())
        }

        async fn search_documents(
            &self,
            query: crate::domain::reference::SearchQuery,
            _score_threshold: Option<f32>,
        ) -> anyhow::Result<Vec<SearchResult>> { // Use anyhow::Result
            log::info!(
                "MockReferenceService: search_documents called with query: {:?}",
                query
            );
            let results = self.results_to_return.lock().unwrap().clone();
            Ok(results)
        }

        fn search(
            &self,
            _collection_name: String,
            _vector: Vec<f32>,
            _limit: u64,
        ) -> BoxFuture<Result<Vec<ScoredPoint>, String>> { // Keep original return type for trait impl
            unimplemented!("Mock search not needed for these tests")
        }

        fn upsert(
            &self,
            _collection_name: String,
            _points: Vec<PointStruct>,
        ) -> BoxFuture<Result<(), String>> { // Keep original return type for trait impl
            unimplemented!("Mock upsert not needed for these tests")
        }
    }

    // Helper function specific to handler unit tests
    fn setup_handler_for_unit_test() -> (McpServerHandler, Arc<MockReferenceService>) {
        let mock_service = Arc::new(MockReferenceService::default());
        let handler = McpServerHandler {
            reference_service_state: Arc::new(Mutex::new(Some(mock_service.clone()))),
            config: Arc::new(McpConfig::default()), // Use default config for unit tests
        };
        (handler, mock_service)
    }

     // Helper function specific to handler unit tests with None state
    fn setup_handler_initializing_for_unit_test() -> McpServerHandler {
        McpServerHandler {
            reference_service_state: Arc::new(Mutex::new(None)), // Start as None
            config: Arc::new(McpConfig::default()),
        }
    }


    // --- Unit Tests for Handler Logic --- //

    #[tokio::test]
    async fn test_search_docs_semantic_rich_output_tool() {
        let (handler, mock_service) = setup_handler_for_unit_test(); // Use unit test helper
        let mock_result = SearchResult {
            file_path: "a.md".to_string(),
            score: 0.85,
            source: Some("SourceA".to_string()),
            content_chunk: "Chunk 1 content".to_string(),
            metadata: Some(json!({ "tag": "test" })),
            document_content: Some("Full document content for a.md".to_string()),
        };
        mock_service.set_search_results(vec![mock_result.clone()]);

        let args = SearchDocsArgs {
            query: "test query".to_string(),
            limit: Some(3),
        };
        let result = handler
            .mc_search_docs_semantic(args)
            .await
            .expect("Tool call failed");

        assert_eq!(result.is_error, Some(false));
        assert_eq!(result.content.len(), 1);

        let annotated_content = &result.content[0];
        match &annotated_content.raw {
            RawContent::Text(text) => {
                let parsed_results: Vec<SearchResult> = serde_json::from_str(&text.text)
                    .expect("Failed to parse SearchResult from text content");
                assert_eq!(parsed_results.len(), 1);
                assert_eq!(parsed_results[0], mock_result); // Direct comparison if SearchResult derives PartialEq
            }
            other_kind => panic!("Expected Text content, found {:?}", other_kind),
        }
    }

    #[tokio::test]
    async fn test_search_docs_semantic_no_results_output_tool() {
        let (handler, mock_service) = setup_handler_for_unit_test();
        mock_service.set_search_results(vec![]); // No results

        let args = SearchDocsArgs {
            query: "nonexistent".to_string(),
            limit: Some(3),
        };
        let result = handler
            .mc_search_docs_semantic(args)
            .await
            .expect("Tool call failed");

        assert_eq!(result.is_error, Some(false));
        assert_eq!(result.content.len(), 1);

        let annotated_content = &result.content[0];
        match &annotated_content.raw {
            RawContent::Text(text) => {
                let parsed_results: Vec<SearchResult> = serde_json::from_str(&text.text)
                    .expect("Failed to parse SearchResult from text content");
                assert!(parsed_results.is_empty());
            }
            other_kind => panic!("Expected Text content, found {:?}", other_kind),
        }
    }


    #[tokio::test]
    async fn test_search_docs_semantic_initializing() {
        let handler = setup_handler_initializing_for_unit_test(); // Use specific helper

        let args = SearchDocsArgs {
            query: "any query".to_string(),
            limit: None,
        };
        let result = handler
            .mc_search_docs_semantic(args)
            .await
            .expect("Tool call failed");

        assert_eq!(result.is_error, Some(true)); // Expect error status
        assert_eq!(result.content.len(), 1);

        let error_text = &result.content[0].raw.as_text().expect("Expected text").text;
        assert!(error_text.contains("service is still initializing"));
    }
}

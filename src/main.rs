// NOTE: use statements will need adjustment after refactoring
use rmcp::serde_json::json;
use rmcp::{
    model::{
        CallToolResult, Content, GetPromptRequestParam, GetPromptResult, Implementation,
        ListPromptsResult, ListResourceTemplatesResult, ListResourcesResult, PaginatedRequestParam,
        ProtocolVersion, ReadResourceRequestParam, ReadResourceResult,
        ServerCapabilities, ServerInfo,
    },
    schemars::{self, JsonSchema},
    service::RequestContext,
    tool, Error as McpError, RoleServer, ServerHandler, ServiceExt,
};
use serde::Deserialize;
use std::sync::Arc;
use tokio::{
    io::{stdin, stdout},
    process::Command,
};

// Import from the library crate using its name (mc-mcp)
use mc_mcp::application::reference_service::ReferenceServiceImpl;
use mc_mcp::config; // Import config module directly
 // Import DocumentSource for MockReferenceService
use mc_mcp::domain; // Import domain for SearchQuery
use mc_mcp::domain::reference::ReferenceService;
use mc_mcp::domain::vector_repository::VectorRepository;
use mc_mcp::file_system; // Import file_system module directly
use mc_mcp::infrastructure::embedding::EmbeddingGenerator;
use mc_mcp::infrastructure::file_system::download_if_not_exists;
use mc_mcp::infrastructure::vector_db::{qdrant_client, VectorDb};
use mc_mcp::infrastructure::EmbeddingModel;

use anyhow::Result;
use env_logger;
use log;
// Add imports for Qdrant types used in mock impl
// Import BoxFuture for mock impl signature

use std::thread::sleep;
use std::time::Duration;
use std::path::PathBuf; // Add PathBuf import

// Import McpConfig from the library crate
use mc_mcp::config::McpConfig;

const PREBUILT_INDEX_URL: &str =
    "https://github.com/metacontract/mc-mcp/releases/latest/download/prebuilt_index.jsonl.gz";
const MC_TEMPLATE_REPO: &str = "metacontract/template";

#[tokio::main]
async fn main() -> Result<()> {
    // check if qdrant is running
    if let Err(e) = ensure_qdrant_via_docker() {
        eprintln!("{e}");
        std::process::exit(1);
    }
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("trace"))
        .target(env_logger::Target::Stderr)
        .init();
    log::info!("mc-mcp server (MCP over stdio) started.");

    // --- Load Configuration ---
    let config = config::load_config()?;
    log::info!("Configuration loaded: {:?}", config);
    let config_arc = Arc::new(config); // Create Arc for sharing

    // --- Download Prebuilt Index (If Configured and Not Exists) ---
    // Moved this block *before* loading/DI setup that might use the index
    if let Some(dest_path_buf) = &config_arc.reference.prebuilt_index_path { // Get path from config
        log::info!("Checking/Downloading prebuilt index to {:?}...", dest_path_buf);
        // download_if_not_exists takes &str, so convert PathBuf
        if let Some(dest_str) = dest_path_buf.to_str() {
            // Ensure the parent directory exists before attempting download
            if let Some(parent_dir) = dest_path_buf.parent() {
                if !parent_dir.exists() {
                    if let Err(e) = std::fs::create_dir_all(parent_dir) {
                        log::error!("Failed to create directory for prebuilt index {:?}: {}", parent_dir, e);
                        // Decide if we should continue or error out - let's log and continue for now
                    }
                }
            }

            match download_if_not_exists(PREBUILT_INDEX_URL, dest_str) {
                Ok(_) => log::info!("Checked/Downloaded prebuilt index to {:?}", dest_path_buf),
                Err(e) => log::error!(
                    "Failed to check/download prebuilt index to {:?}: {}",
                    dest_path_buf, e
                    // Log error but continue, index loading will handle the missing file
                ),
            }
        } else {
            log::error!(
                "Configured prebuilt index path is not valid UTF-8: {:?}",
                dest_path_buf
            );
        }
    } else {
        log::info!("Skipping prebuilt index download check as no path is configured.");
    }

    // --- Dependency Injection Setup (Moved earlier to access vector_db) ---
    let qdrant_url =
        std::env::var("QDRANT_URL").unwrap_or_else(|_| "http://localhost:6334".to_string());
    let qdrant_client = qdrant_client::Qdrant::from_url(&qdrant_url).build()?;
    let collection_name =
        std::env::var("QDRANT_COLLECTION").unwrap_or_else(|_| "mc_docs".to_string());
    let vector_dim: u64 = std::env::var("EMBEDDING_DIM")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(384);
    let embedding_model = EmbeddingModel::AllMiniLML6V2;
    let embedder = Arc::new(EmbeddingGenerator::new(embedding_model)?);
    let vector_db_instance =
        VectorDb::new(Box::new(qdrant_client), collection_name.clone(), vector_dim)?;
    // Initialize collection *before* potentially upserting prebuilt index
    vector_db_instance.initialize_collection().await?;
    // Get Arc<dyn VectorRepository> to use for both prebuilt index and service
    let vector_db: Arc<dyn VectorRepository> = Arc::new(vector_db_instance);
    // Keep reference_service initialization separate for now
    // let reference_service: Arc<dyn ReferenceService> = Arc::new(ReferenceServiceImpl::new(embedder.clone(), vector_db.clone()));

    // --- Prebuilt Index Loading (If configured) ---
    // Now this runs *after* the download attempt
    if let Some(prebuilt_path) = &config_arc.reference.prebuilt_index_path {
        log::info!(
            "Attempting to load prebuilt index from: {:?}",
            prebuilt_path
        );
        match file_system::load_prebuilt_index(prebuilt_path.clone()) {
            Ok(prebuilt_docs) => {
                log::info!(
                    "Successfully loaded {} documents from prebuilt index.",
                    prebuilt_docs.len()
                );
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
    let reference_service = Arc::new(ReferenceServiceImpl::new(embedder, vector_db.clone()));

    // --- Initial Indexing from configured sources (after potential prebuilt index loading) ---
    log::info!(
        "Triggering indexing from configured sources ({} sources)...",
        config_arc.reference.sources.len()
    );
    // Decide if we should always index sources, or skip if prebuilt was loaded?
    // For now, let's always index configured sources after loading prebuilt.
    // Duplicates might be overwritten depending on VectorDb implementation.
    if let Err(e) = reference_service
        .index_sources(&config_arc.reference.sources)
        .await
    {
        log::error!("Error during configured source indexing: {}", e);
    }
    log::info!("Configured source indexing process started/completed.");

    // --- Start MCP Server ---
    let handler = MyHandler { reference_service, config: config_arc.clone() }; // Pass Arc to handler
    let transport = (stdin(), stdout());

    log::info!("Starting MCP server...");
    log::trace!("Just before handler.serve() call"); // New Log 1
    let serve_future = handler.serve(transport);
    log::trace!("handler.serve() called, future created. Before .await"); // New Log 2
    let server_handle = serve_future.await.inspect_err(|e| {
        log::error!("serving error: {:?}", e);
    })?;
    log::trace!("handler.serve().await returned successfully."); // Renamed previous log

    log::info!("mc-mcp server running, waiting for completion...");
    let shutdown_reason = server_handle.waiting().await?;
    log::info!("mc-mcp server finished. Reason: {:?}", shutdown_reason);

    // --- Download prebuilt index if configured and not exists --- << REMOVE THIS BLOCK
    /*
    if let Some(dest_path_buf) = &config_arc.reference.prebuilt_index_path { // Get path from config
        // download_if_not_exists takes &str, so convert PathBuf
        if let Some(dest_str) = dest_path_buf.to_str() {
            match download_if_not_exists(PREBUILT_INDEX_URL, dest_str) {
                Ok(_) => log::info!("Checked/Downloaded prebuilt index to {:?}", dest_path_buf),
                Err(e) => log::error!(
                    "Failed to check/download prebuilt index to {:?}: {}",
                    dest_path_buf, e
                ),
            }
        } else {
            log::error!(
                "Configured prebuilt index path is not valid UTF-8: {:?}",
                dest_path_buf
            );
        }
    } else {
        log::info!("Skipping prebuilt index download check as no path is configured.");
    }
    */

    Ok(())
}

/// Handler for the MCP server logic.
#[derive(Clone)]
struct MyHandler {
    reference_service: Arc<dyn ReferenceService>,
    config: Arc<McpConfig>,
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

#[tool(tool_box)]
impl MyHandler {
    /// Creates a new handler instance.
    fn new(reference_service: Arc<dyn ReferenceService>, config: Arc<McpConfig>) -> Self {
        Self { reference_service, config }
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
                log::debug!("forge stdout:\n{}", stdout);
                log::debug!("forge stderr:\n{}", stderr);

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
                    "{}:\nScript: {}\nBroadcast: {}\nExit Code: {}\n\nStdout:\n{}\nStderr:\n{}",
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
                    "Forge Test Results:\nExit Code: {}\n\nStdout:\n{}\nStderr:\n{}",
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

        let search_query = domain::reference::SearchQuery {
            text: query.clone(),
            limit: Some(limit),
            sources: None,
        };
        match self
            .reference_service
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
                            let err_msg =
                                format!("Failed to create text response from JSON: {:?}", e);
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
    }

    /// Initializes a new Foundry project using the metacontract template.
    /// This tool only works if the current directory is empty.
    /// It runs `forge init . -t metacontract/template`.
    #[tool(
        description = "Initialize a new Foundry project using a template. Only works in an empty directory."
    )]
    pub async fn mc_setup(&self) -> Result<CallToolResult, McpError> {
        use std::env;
        use std::fs;
        use std::process::Command;
        // 1. Check if current directory is empty
        let current_dir = env::current_dir().map_err(|e| {
            McpError::internal_error(format!("Failed to get current dir: {e}"), None)
        })?;
        let entries = fs::read_dir(&current_dir).map_err(|e| {
            McpError::internal_error(format!("Failed to read current dir: {e}"), None)
        })?;
        let is_empty = entries.into_iter().next().is_none();
        if !is_empty {
            return Ok(CallToolResult::error(vec![Content::text(
                "The current directory is not empty. Please run setup in a new directory.",
            )]));
        }
        // 2. Run forge init . -t <repo>
        // テンプレートキャッシュが指定されていればローカルパスを使う
        let template_arg = if let Ok(local_template) = std::env::var("MC_TEMPLATE_CACHE") {
            // Foundryはローカルパスも-tで受け付ける
            local_template
        } else {
            MC_TEMPLATE_REPO.to_string()
        };

        // Use output() to capture stderr as well
        let output_result = Command::new("forge")
            .args(["init", ".", "-t", &template_arg])
            .output(); // Use output() instead of status(), remove .await and ?

        match output_result { // Use match to handle the Result<Output, Error>
            Ok(output_result) => {
                if output_result.status.success() {
                    Ok(CallToolResult::success(vec![Content::text(format!(
                        "Successfully initialized Foundry project with template: {MC_TEMPLATE_REPO}"
                    ))]))
                } else {
                    let stderr = String::from_utf8_lossy(&output_result.stderr).to_string();
                    let err_msg = format!(
                        "forge init failed with exit code: {:?}.\nStderr:\n{}",
                        output_result.status.code(),
                        stderr
                    );
                    log::error!("mc_setup forge init failed: {}", err_msg);
                    Ok(CallToolResult::error(vec![Content::text(err_msg)]))
                }
            }
            Err(e) => { // Handle the command execution error explicitly
                let err_msg = format!("Failed to execute forge init command: {}. Make sure 'forge' is installed and in PATH.", e);
                log::error!("mc_setup failed to execute forge: {}", e);
                Ok(CallToolResult::error(vec![Content::text(err_msg)])) // Return CallToolResult::error
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
}

#[tool(tool_box)]
impl ServerHandler for MyHandler {
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
    use std::sync::{Arc, Mutex};

    // Use the library crate path for domain items in tests
    use anyhow::Result;

    use mc_mcp::config::DocumentSource; // Import DocumentSource via library crate
    use mc_mcp::domain::reference::SearchResult;
    use mc_mcp::ReferenceService; // Import trait via library crate


    use rmcp::serde_json::{self, json};
    use futures::future::BoxFuture;
    use qdrant_client::qdrant::{PointStruct, ScoredPoint};
    use rmcp::model::RawContent; // Import RawContent only

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
            println!(
                "MockReferenceService: index_sources called with {} sources",
                sources.len()
            );
            self.indexed_sources
                .lock()
                .unwrap()
                .extend_from_slice(sources);
            Ok(())
        }
        // search_documents implementation
        async fn search_documents(
            &self,
            query: mc_mcp::domain::reference::SearchQuery,
            _score_threshold: Option<f32>,
        ) -> Result<Vec<SearchResult>> {
            println!(
                "MockReferenceService: search_documents called with query: {:?}",
                query
            );
            let results = self.results_to_return.lock().unwrap().clone();
            Ok(results)
        }
        // Dummy implementations for non-async trait methods (required by compiler)
        fn search(
            &self,
            _collection_name: String,
            _vector: Vec<f32>,
            _limit: u64,
        ) -> BoxFuture<Result<Vec<ScoredPoint>, String>> {
            unimplemented!("Mock search not needed for these tests")
        }
        fn upsert(
            &self,
            _collection_name: String,
            _points: Vec<PointStruct>,
        ) -> BoxFuture<Result<(), String>> {
            unimplemented!("Mock upsert not needed for these tests")
        }
    }

    // --- Test Setup --- //
    // Helper function returns concrete mock type now
    fn setup_mock_handler() -> (MyHandler, Arc<MockReferenceService>) {
        let mock_service = Arc::new(MockReferenceService::default());
        let handler = MyHandler {
            reference_service: mock_service.clone(),
            config: Arc::new(McpConfig {
                scripts: mc_mcp::config::ScriptsConfig {
                    deploy: Some("scripts/Deploy.s.sol".to_string()), // Default for tests
                    upgrade: Some("scripts/Upgrade.s.sol".to_string()), // Default upgrade script for tests
                    rpc_url: None, // Add default None for tests
                    private_key_env_var: None, // Add default None for tests
                },
                ..Default::default()
            }),
        };
        (handler, mock_service)
    }

    // Helper to setup mock handler with specific config
    fn setup_mock_handler_with_config(
        config: McpConfig,
    ) -> (MyHandler, Arc<MockReferenceService>) {
        let mock_service = Arc::new(MockReferenceService::default());
        let handler = MyHandler {
            reference_service: mock_service.clone(),
            config: Arc::new(config), // Pass specific config
        };
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

        // Match on annotated_content.raw
        let annotated_content = &result.content[0];
        match &annotated_content.raw {
            // Match on .raw field
            RawContent::Text(text) => {
                // Expect Text containing JSON string
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
            other_kind => panic!(
                "Expected Text content containing JSON, found {:?}",
                other_kind
            ),
        }
    }

    #[tokio::test]
    async fn test_search_docs_semantic_no_results_output_tool() {
        let (handler, mock_service) = setup_mock_handler();
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

        // Match on annotated_content.raw
        let annotated_content = &result.content[0];
        match &annotated_content.raw {
            // Match on .raw field
            RawContent::Text(text) => {
                // Expect Text containing JSON string
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
            other_kind => panic!(
                "Expected Text content containing JSON, found {:?}",
                other_kind
            ),
        }
    }

    #[tokio::test]
    #[ignore] // Ignore this test for now, requires forge setup that succeeds
    async fn test_forge_test_tool_runs_successfully() {
        let (handler, _mock_service) = setup_mock_handler();

        // Assume forge command exists and runs without internal errors for this test
        let result = handler.mc_test().await.expect("Tool call should not panic");

        // Expect success (is_error = false) when forge command runs
        assert_eq!(
            result.is_error,
            Some(false),
            "Expected is_error to be false when forge runs"
        );
        assert!(
            !result.content.is_empty(),
            "Expected content to be non-empty"
        );

        // Optional: Further check content if needed, e.g., look for typical success output
        let annotated_content = &result.content[0];
        let _ = match &annotated_content.raw {
            // Match on .raw field and bind to _
            RawContent::Text(text) => {
                // We can't reliably assert on the exact output, but we know it should be text
                assert!(!text.text.is_empty(), "Expected non-empty text output");
            }
            other_kind => panic!("Expected Text content, found {:?}", other_kind),
        };
    }

    #[tokio::test]
    // This test should pass in environments where `forge test` runs but fails internally
    async fn test_forge_test_tool_reports_failure() {
        let (handler, _mock_service) = setup_mock_handler();

        // Assume forge command exists but fails internally (non-zero exit code)
        let result = handler.mc_test().await.expect("Tool call should not panic");

        // Expect error (is_error = true) when forge command fails internally
        assert_eq!(
            result.is_error,
            Some(true),
            "Expected is_error to be true when forge fails"
        );
        assert!(
            !result.content.is_empty(),
            "Expected content to be non-empty"
        );

        // Optional: Check for specific failure messages in content if needed
        let annotated_content = &result.content[0];
        let _ = match &annotated_content.raw {
            // Match on .raw field and bind to _
            RawContent::Text(text) => {
                assert!(
                    !text.text.is_empty(),
                    "Expected non-empty text output for failure"
                );
            }
            other_kind => panic!("Expected Text content for failure, found {:?}", other_kind),
        };
    }

    #[tokio::test]
    async fn test_setup_empty_dir() {
        let (handler, _) = setup_mock_handler();
        // Create a temporary empty directory for the test
        let temp_dir = tempfile::tempdir().unwrap();

        // Define path to mock_bin relative to project root *before* changing CWD
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let mock_bin_path = std::path::Path::new(manifest_dir).join("tests/mock_bin");

        // Ensure mock_bin directory exists
        if !mock_bin_path.exists() {
            std::fs::create_dir_all(&mock_bin_path).unwrap(); // Use create_dir_all
        }

        // Create a mock forge script in mock_bin
        let forge_script_path = mock_bin_path.join("forge");
        #[cfg(unix)]
        std::fs::write(&forge_script_path, "#!/bin/sh\nexit 0").unwrap();
        #[cfg(windows)]
        std::fs::write(&forge_script_path, "exit 0").unwrap();

        // Make it executable on Unix-like systems
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&forge_script_path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&forge_script_path, perms).unwrap();
        }

        // Store original PATH and CWD
        let original_path = std::env::var("PATH").unwrap_or_default();
        let original_cwd = std::env::current_dir().unwrap();

        // Change CWD to temp directory for the test logic
        std::env::set_current_dir(temp_dir.path()).unwrap();

        // Prepend mock_bin directory to PATH
        let mock_bin_abs_path = mock_bin_path.canonicalize().unwrap();
        std::env::set_var("PATH", format!("{}:{}", mock_bin_abs_path.display(), original_path));

        // Run the setup tool
        let result = handler.mc_setup().await;

        // Assertions
        assert!(result.is_ok());
        let call_result = result.unwrap();
        assert_eq!(call_result.is_error, Some(false), "Expected success");

        // Cleanup
        std::env::set_var("PATH", original_path);
        std::env::set_current_dir(original_cwd).unwrap();

        // Clean up mock script and dir (optional)
        // std::fs::remove_file(forge_script_path).ok();
        // std::fs::remove_dir_all(&mock_bin_path).ok();
    }

    #[tokio::test]
    async fn test_setup_non_empty_dir() {
        let (handler, _) = setup_mock_handler();
        // Create a temporary non-empty directory
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(temp_dir.path().join("dummy.txt"), "hello").unwrap();
        let original_dir = std::env::current_dir().unwrap(); // Store original dir
        std::env::set_current_dir(temp_dir.path()).unwrap();

        let result = handler.mc_setup().await; // Updated call
        assert!(result.is_ok());
        let call_result = result.unwrap();
        // Use is_error instead of status
        assert_eq!(call_result.is_error, Some(true), "Expected error");
        assert_eq!(call_result.content.len(), 1, "Expected one content item");
        // Access text content correctly using a reference
        let content_text = &call_result.content[0].raw.as_text().expect("Expected text content").text;
        assert!(content_text.contains("The current directory is not empty"));

        // Restore original directory
        std::env::set_current_dir(original_dir).unwrap();
    }

    #[tokio::test]
    async fn test_mc_deploy_dry_run_success() {
        // Ensure we are in the project root directory before starting the test
        let manifest_dir_check = env!("CARGO_MANIFEST_DIR");
        std::env::set_current_dir(manifest_dir_check).expect("Failed to set current dir to project root");

        let expected_script_path = "scripts/Deploy.s.sol"; // Path from default mock config
        let (handler, _mock_service) = setup_mock_handler(); // Uses default mock config

        // Setup mock forge script to expect the default path for deploy dry run
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let mock_bin_path = std::path::Path::new(manifest_dir).join("tests/mock_bin");
        if !mock_bin_path.exists() {
            std::fs::create_dir_all(&mock_bin_path).unwrap();
        }
        let forge_script_path_mock = mock_bin_path.join("forge");
        let mock_script_content = format!(
            r#"#!/bin/sh
# Check arguments: script <path> (no broadcast)
if [ "$1" = "script" ] && [ "$2" = "{}" ] && [ "$#" -eq 2 ]; then
  echo "Deploy Dry run successful for {}"
  exit 0 # Exit successfully
else
  echo "Error: Unexpected mock forge call arguments in deploy dry run success test: $@" >&2
  exit 1
fi
"#,
            expected_script_path, expected_script_path
        );
        #[cfg(unix)]
        std::fs::write(&forge_script_path_mock, mock_script_content).unwrap();
        #[cfg(windows)]
        std::fs::write(&forge_script_path_mock, "@echo off\necho Deploy Dry run successful\nexit /b 0").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&forge_script_path_mock).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&forge_script_path_mock, perms).unwrap();
        }

        let original_path = std::env::var("PATH").unwrap_or_default();
        let mock_bin_abs_path = mock_bin_path.canonicalize().unwrap();
        std::env::set_var("PATH", format!("{}:{}", mock_bin_abs_path.display(), original_path));

        let args = McDeployArgs {
            broadcast: Some(false), // Dry run
        };

        let result = handler.mc_deploy(args).await;

        // --- Assertions --- //
        assert!(result.is_ok(), "mc_deploy call failed: {:?}", result.err());
        let call_result = result.unwrap();
        assert_eq!(call_result.is_error, Some(false), "Expected success status");
        assert!(!call_result.content.is_empty(), "Expected output content");

        let output_text = &call_result.content[0].raw.as_text().expect("Expected text content").text;
        assert!(output_text.contains("Deploy Dry Run Results"), "Output should mention deploy dry run");
        assert!(output_text.contains(expected_script_path), "Output should mention script path");

        // Cleanup
        std::env::set_var("PATH", original_path);
    }

    #[tokio::test]
    async fn test_mc_deploy_broadcast_success() {
        // Ensure we are in the project root directory before starting the test
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        std::env::set_current_dir(manifest_dir).expect("Failed to set current dir to project root");

        let expected_script_path = "scripts/Deploy.s.sol";
        let expected_rpc_url = "http://localhost:8545";
        let expected_pk_env = "TEST_PRIVATE_KEY";

        // --- Create config with broadcast settings ---
        let config = McpConfig {
            scripts: mc_mcp::config::ScriptsConfig {
                deploy: Some(expected_script_path.to_string()),
                upgrade: Some("scripts/Upgrade.s.sol".to_string()), // Need some upgrade path
                rpc_url: Some(expected_rpc_url.to_string()),
                private_key_env_var: Some(expected_pk_env.to_string()),
            },
            ..Default::default()
        };
        let (handler, _mock_service) = setup_mock_handler_with_config(config);
        // --- End config setup ---

        let mock_bin_path = std::path::Path::new(manifest_dir).join("tests/mock_bin");
        // --- Ensure mock_bin directory exists ---
        if !mock_bin_path.exists() {
            std::fs::create_dir_all(&mock_bin_path).expect("Failed to create mock_bin dir");
        }
        let forge_script_on_mock_bin = mock_bin_path.join("forge"); // パスだけ作る

        // --- Define and write the mock script for this specific test ---
        let mock_script_content_broadcast = format!(
            r#"#!/bin/sh
echo "Mock deploy broadcast script called with: $@" >&2 # Log arguments
# Check arguments: script <deploy_path> --broadcast --rpc-url <url> --private-key $<env_var>
if [ "$1" = "script" ] && \
   [ "$2" = "{}" ] && \
   [ "$3" = "--broadcast" ] && \
   [ "$4" = "--rpc-url" ] && [ "$5" = "{}" ] && \
   [ "$6" = "--private-key" ] && [ "$7" = "\${}" ] && \
   [ "$#" -eq 7 ]; then
  echo "Simulating deploy broadcast..."
  echo "Transaction Hash: 0xmockhashbroadcast" # Mock output
  exit 0 # Exit successfully
else
  echo "Error: Unexpected mock forge call arguments in deploy broadcast success test: $@" >&2
  exit 1
fi
"#,
            expected_script_path, expected_rpc_url, expected_pk_env
        );
        #[cfg(unix)]
        std::fs::write(&forge_script_on_mock_bin, mock_script_content_broadcast).expect("Failed to write mock deploy broadcast script");
        #[cfg(windows)] // Basic windows version for completeness
        std::fs::write(&forge_script_on_mock_bin, "@echo off\necho Transaction Hash: 0xmockhashbroadcast\nexit /b 0").expect("Failed to write mock deploy broadcast script (win)");
        // --- End mock script definition ---


        // --- Debug: Check if mock script file actually exists ---
        println!("Checking existence of: {}", forge_script_on_mock_bin.display());
        if forge_script_on_mock_bin.exists() {
            println!("Mock script file FOUND.");

            // --- Add execute permission ---
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                println!("Setting execute permission on mock script...");
                let metadata = std::fs::metadata(&forge_script_on_mock_bin).expect("Failed to get metadata for mock script");
                let mut perms = metadata.permissions();
                if perms.mode() & 0o111 == 0 { // Check if execute bit is NOT set
                    perms.set_mode(perms.mode() | 0o111); // Add execute permission for user, group, others
                    std::fs::set_permissions(&forge_script_on_mock_bin, perms).expect("Failed to set permissions on mock script");
                    println!("Execute permission set.");
                } else {
                    println!("Execute permission already set.");
                }
            }
            // --- End Add execute permission ---

        } else {
            println!("Mock script file NOT FOUND!");
            // もし見つからなかったら、ls の結果も見てみる
            let ls_output = std::process::Command::new("ls").arg("-al").arg(mock_bin_path.display().to_string()).output();
            match ls_output {
                Ok(out) => println!("ls -al {}:\n{}", mock_bin_path.display(), String::from_utf8_lossy(&out.stdout)),
                Err(e) => println!("Failed to run ls: {}", e),
            }
        }
        // --- End Debug ---

        // canonicalize は存在チェックの後に行う
        let forge_script_path_mock_abs = forge_script_on_mock_bin.canonicalize().expect("Failed to canonicalize mock forge script path");
        let original_path = std::env::var("PATH").unwrap_or_default();
        let mock_bin_abs_path_str = mock_bin_path.canonicalize().unwrap().display().to_string();
        std::env::set_var("PATH", format!("{}:{}", mock_bin_abs_path_str, original_path));

        // --- Debugging: Check which forge is used and run mock script directly ---
        println!("--- Debug Info: test_mc_deploy_broadcast_success ---");
        let which_output = std::process::Command::new("which").arg("forge").output().expect("Failed to run which forge");
        println!("which forge stdout: {}", String::from_utf8_lossy(&which_output.stdout));
        println!("which forge stderr: {}", String::from_utf8_lossy(&which_output.stderr));
        println!("which forge status: {:?}", which_output.status.code());

        println!("Executing mock script directly: {} script {} --broadcast", forge_script_path_mock_abs.display(), expected_script_path);
        let mock_run_output = std::process::Command::new(forge_script_path_mock_abs)
            .arg("script")
            .arg(expected_script_path)
            .arg("--broadcast")
            .output()
            .expect("Failed to run mock forge script directly");
        println!("Mock script stdout: {}", String::from_utf8_lossy(&mock_run_output.stdout));
        println!("Mock script stderr: {}", String::from_utf8_lossy(&mock_run_output.stderr));
        println!("Mock script status: {:?}", mock_run_output.status.code());
        println!("--- End Debug Info ---");
        // --- End Debugging ---

        let args = McDeployArgs {
            broadcast: Some(true),
        };

        let result = handler.mc_deploy(args).await;

        // --- Assertions --- //
        assert!(result.is_ok(), "mc_deploy call failed: {:?}", result.err());
        let call_result = result.unwrap();
        // This assertion is currently failing
        assert_eq!(call_result.is_error, Some(false), "Expected success status");
        assert!(!call_result.content.is_empty(), "Expected output content");

        let output_text = &call_result.content[0].raw.as_text().expect("Expected text content").text;
        assert!(output_text.trim_start().starts_with("Forge Deploy Results:"), "Output should start with deploy results title");
        assert!(output_text.contains("Exit Code: 0"), "Should show exit code 0 for success"); // Check for Exit Code 0
        assert!(output_text.contains(&format!("Script: {}", expected_script_path)), "Output should mention script path");
        assert!(output_text.contains("Broadcast: true"), "Output should mention broadcast true");
        assert!(output_text.contains("Transaction Hash: 0xmockhashbroadcast"), "Output should contain mock deploy tx hash");

        // Cleanup
        std::env::set_var("PATH", original_path);
    }

    // Test for deploy broadcast failure
    #[tokio::test]
    async fn test_mc_deploy_broadcast_failure() {
        let manifest_dir_check = env!("CARGO_MANIFEST_DIR");
        std::env::set_current_dir(manifest_dir_check).expect("Failed to set current dir to project root");

        let expected_script_path = "scripts/Deploy.s.sol";
        // --- Provide valid config for this test case ---
        let config = McpConfig {
            scripts: mc_mcp::config::ScriptsConfig {
                deploy: Some(expected_script_path.to_string()),
                upgrade: Some("scripts/Upgrade.s.sol".to_string()), // Need some path
                rpc_url: Some("http://localhost:8545".to_string()),
                private_key_env_var: Some("TEST_PK_ENV".to_string()),
            },
            ..Default::default()
        };
        let (handler, _mock_service) = setup_mock_handler_with_config(config);
        // --- End config setup ---

        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let mock_bin_path = std::path::Path::new(manifest_dir).join("tests/mock_bin");
        if !mock_bin_path.exists() {
            std::fs::create_dir_all(&mock_bin_path).unwrap();
        }
        let forge_script_path_mock = mock_bin_path.join("forge");
        // --- Mock script MUST exit with 1 for failure test ---
        let mock_script_content = format!(
            r#"#!/bin/sh
# This mock should simulate a failed broadcast, so it exits with 1
echo "Mock deploy broadcast failure called with: $@" >&2 # Log args
if [ "$1" = "script" ] && [ "$2" = "{}" ] && [ "$3" = "--broadcast" ]; then
  echo "Error: Broadcast failed due to network issue." >&2 # Output to stderr
  exit 1 # Exit with error code
else
  echo "Unexpected mock forge call in deploy broadcast failure test: $@" >&2
  exit 1 # Also exit with error code if args mismatch
fi
"#,
            expected_script_path
        );
        #[cfg(unix)]
        std::fs::write(&forge_script_path_mock, mock_script_content).unwrap();
        #[cfg(windows)] // Basic windows version
        std::fs::write(&forge_script_path_mock, "@echo off\necho Error: Broadcast failed due to network issue. >&2\nexit /b 1").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&forge_script_path_mock).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&forge_script_path_mock, perms).unwrap();
        }

        let original_path = std::env::var("PATH").unwrap_or_default();
        let mock_bin_abs_path = mock_bin_path.canonicalize().unwrap();
        std::env::set_var("PATH", format!("{}:{}", mock_bin_abs_path.display(), original_path));

        let args = McDeployArgs { broadcast: Some(true) }; // Broadcast
        let result = handler.mc_deploy(args).await;

        assert!(result.is_ok());
        let call_result = result.unwrap();
        assert_eq!(call_result.is_error, Some(true), "Expected error status for failed broadcast");
        assert!(!call_result.content.is_empty());

        let output_text = &call_result.content[0].raw.as_text().expect("Expected text").text;
        assert!(output_text.trim_start().starts_with("Forge Deploy Results:"), "Output should start with deploy results title");
        assert!(output_text.contains("Exit Code: 1"), "Should show exit code 1");
        assert!(output_text.contains("Error: Broadcast failed due to network issue."), "Should contain stderr message");

        std::env::set_var("PATH", original_path);
    }

    // --- mc_upgrade Tests --- //

    #[tokio::test]
    async fn test_mc_upgrade_dry_run_success() {
        // Ensure we are in the project root directory before starting the test
        let manifest_dir_check = env!("CARGO_MANIFEST_DIR");
        std::env::set_current_dir(manifest_dir_check).expect("Failed to set current dir to project root");

        let expected_script_path = "scripts/Upgrade.s.sol"; // Path from default mock config
        let (handler, _mock_service) = setup_mock_handler(); // Uses default mock config

        // Setup mock forge script to expect the default path for upgrade dry run
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let mock_bin_path = std::path::Path::new(manifest_dir).join("tests/mock_bin");
        if !mock_bin_path.exists() {
            std::fs::create_dir_all(&mock_bin_path).unwrap();
        }
        let forge_script_path_mock = mock_bin_path.join("forge");
        let mock_script_content = format!(
            r#"#!/bin/sh
# Check arguments: script <path> (no broadcast)
if [ "$1" = "script" ] && [ "$2" = "{}" ] && [ "$#" -eq 2 ]; then
  echo "Upgrade Dry run successful for {}"
  exit 0 # Exit successfully
else
  echo "Error: Unexpected mock forge call arguments in upgrade dry run success test: $@" >&2
  exit 1
fi
"#,
            expected_script_path, expected_script_path
        );
        #[cfg(unix)]
        std::fs::write(&forge_script_path_mock, mock_script_content).unwrap();
        #[cfg(windows)]
        std::fs::write(&forge_script_path_mock, "@echo off\necho Upgrade Dry run successful\nexit /b 0").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&forge_script_path_mock).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&forge_script_path_mock, perms).unwrap();
        }

        let original_path = std::env::var("PATH").unwrap_or_default();
        let mock_bin_abs_path = mock_bin_path.canonicalize().unwrap();
        std::env::set_var("PATH", format!("{}:{}", mock_bin_abs_path.display(), original_path));

        let args = McUpgradeArgs {
            broadcast: Some(false), // Dry run
        };

        let result = handler.mc_upgrade(args).await;

        // --- Assertions --- //
        assert!(result.is_ok(), "mc_upgrade call failed: {:?}", result.err());
        let call_result = result.unwrap();
        assert_eq!(call_result.is_error, Some(false), "Expected success status");
        assert!(!call_result.content.is_empty(), "Expected output content");

        let output_text = &call_result.content[0].raw.as_text().expect("Expected text content").text;
        assert!(output_text.contains("Upgrade Dry Run Results"), "Output should mention upgrade dry run");
        assert!(output_text.contains(expected_script_path), "Output should mention script path");

        // Cleanup
        std::env::set_var("PATH", original_path);
    }

    #[tokio::test]
    async fn test_mc_upgrade_no_script_configured() {
        // Ensure we are in the project root directory before starting the test
        let manifest_dir_check = env!("CARGO_MANIFEST_DIR");
        std::env::set_current_dir(manifest_dir_check).expect("Failed to set current dir to project root");

        // Create config with no upgrade script path
        let config = McpConfig {
            scripts: mc_mcp::config::ScriptsConfig {
                deploy: Some("scripts/Deploy.s.sol".to_string()),
                upgrade: None, // Explicitly None for upgrade
                rpc_url: None, // Add default None
                private_key_env_var: None, // Add default None
            },
            ..Default::default()
        };
        let (handler, _mock_service) = setup_mock_handler_with_config(config);

        let args = McUpgradeArgs { broadcast: Some(false) };
        let result = handler.mc_upgrade(args).await;

        assert!(result.is_ok()); // Tool call itself should succeed
        let call_result = result.unwrap();
        assert_eq!(call_result.is_error, Some(true)); // Should return an error status
        assert!(!call_result.content.is_empty());
        let error_text = &call_result.content[0].raw.as_text().expect("Expected text").text;
        assert!(error_text.contains("Upgrade script path is not configured"));
    }

    // Add test for upgrade dry run failure
    #[tokio::test]
    async fn test_mc_upgrade_dry_run_failure() {
        let manifest_dir_check = env!("CARGO_MANIFEST_DIR");
        std::env::set_current_dir(manifest_dir_check).expect("Failed to set current dir to project root");

        let expected_script_path = "scripts/Upgrade.s.sol";
        let (handler, _mock_service) = setup_mock_handler();

        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let mock_bin_path = std::path::Path::new(manifest_dir).join("tests/mock_bin");
        if !mock_bin_path.exists() {
            std::fs::create_dir_all(&mock_bin_path).unwrap();
        }
        let forge_script_path_mock = mock_bin_path.join("forge");
        let mock_script_content = format!(
            r#"#!/bin/sh
if [ "$1" = "script" ] && [ "$2" = "{}" ] && [ "$#" -eq 2 ]; then
  echo "Error: Upgrade dry run script failed simulation." >&2 # Output to stderr
  exit 1 # Exit with error code
else
  echo "Unexpected mock forge call in upgrade dry run failure test: $@" >&2
  exit 1
fi
"#,
            expected_script_path
        );
        #[cfg(unix)]
        std::fs::write(&forge_script_path_mock, mock_script_content).unwrap();
        #[cfg(windows)] // Basic windows version
        std::fs::write(&forge_script_path_mock, "@echo off\necho Error: Upgrade dry run script failed simulation. >&2\nexit /b 1").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&forge_script_path_mock).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&forge_script_path_mock, perms).unwrap();
        }

        let original_path = std::env::var("PATH").unwrap_or_default();
        let mock_bin_abs_path = mock_bin_path.canonicalize().unwrap();
        std::env::set_var("PATH", format!("{}:{}", mock_bin_abs_path.display(), original_path));

        let args = McUpgradeArgs { broadcast: Some(false) }; // Dry run
        let result = handler.mc_upgrade(args).await;

        assert!(result.is_ok());
        let call_result = result.unwrap();
        assert_eq!(call_result.is_error, Some(true), "Expected error status for failed upgrade dry run");
        assert!(!call_result.content.is_empty());

        let output_text = &call_result.content[0].raw.as_text().expect("Expected text").text;
        assert!(output_text.contains("Forge Upgrade Dry Run Results"), "Should indicate upgrade dry run results title");
        assert!(output_text.contains("Exit Code: 1"), "Should show exit code 1");
        assert!(output_text.contains("Error: Upgrade dry run script failed simulation."), "Should contain stderr message");

        std::env::set_var("PATH", original_path);
    }

    // Add test for upgrade broadcast success
    #[tokio::test]
    async fn test_mc_upgrade_broadcast_success() {
        // Ensure we are in the project root directory before starting the test
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        std::env::set_current_dir(manifest_dir).expect("Failed to set current dir to project root");

        let expected_script_path = "scripts/Upgrade.s.sol";
        let expected_rpc_url = "http://localhost:8545";
        let expected_pk_env = "TEST_PRIVATE_KEY";

        // --- Create config with broadcast settings ---
        let config = McpConfig {
            scripts: mc_mcp::config::ScriptsConfig {
                deploy: Some("scripts/Deploy.s.sol".to_string()), // Need some deploy path
                upgrade: Some(expected_script_path.to_string()),
                rpc_url: Some(expected_rpc_url.to_string()),
                private_key_env_var: Some(expected_pk_env.to_string()),
            },
            ..Default::default()
        };
        let (handler, _mock_service) = setup_mock_handler_with_config(config);
        // --- End config setup ---

        let mock_bin_path = std::path::Path::new(manifest_dir).join("tests/mock_bin");
        // --- Ensure mock_bin directory exists ---
        if !mock_bin_path.exists() {
            std::fs::create_dir_all(&mock_bin_path).expect("Failed to create mock_bin dir");
        }
        let forge_script_on_mock_bin = mock_bin_path.join("forge");

        // --- Define and write the mock script for this specific test ---
        let mock_script_content_broadcast = format!(
            r#"#!/bin/sh
echo "Mock upgrade broadcast script called with: $@" >&2 # Log arguments
# Check arguments: script <path> --broadcast --rpc-url <url> --private-key $<env_var>
if [ "$1" = "script" ] && \
   [ "$2" = "{}" ] && \
   [ "$3" = "--broadcast" ] && \
   [ "$4" = "--rpc-url" ] && [ "$5" = "{}" ] && \
   [ "$6" = "--private-key" ] && [ "$7" = "\${}" ] && \
   [ "$#" -eq 7 ]; then
  echo "Simulating upgrade broadcast..."
  echo "Transaction Hash: 0xmockhashupgrade" # Mock output
  exit 0 # Exit successfully
else
  echo "Error: Unexpected mock forge call in upgrade broadcast test: $@" >&2
  exit 1
fi
"#,
            expected_script_path, expected_rpc_url, expected_pk_env
        );
        #[cfg(unix)]
        std::fs::write(&forge_script_on_mock_bin, mock_script_content_broadcast).expect("Failed to write mock upgrade broadcast script");
        #[cfg(windows)]
        std::fs::write(&forge_script_on_mock_bin, "@echo off\necho Transaction Hash: 0xmockhashupgrade\nexit /b 0").expect("Failed to write mock upgrade broadcast script (win)");
        // --- End mock script definition ---


        // --- Debug: Check if mock script file actually exists ---
        println!("Checking existence of: {}", forge_script_on_mock_bin.display());
        if forge_script_on_mock_bin.exists() {
            println!("Mock script file FOUND.");

            // --- Add execute permission ---
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                println!("Setting execute permission on mock script...");
                let metadata = std::fs::metadata(&forge_script_on_mock_bin).expect("Failed to get metadata for mock script");
                let mut perms = metadata.permissions();
                if perms.mode() & 0o111 == 0 { // Check if execute bit is NOT set
                    perms.set_mode(perms.mode() | 0o111); // Add execute permission for user, group, others
                    std::fs::set_permissions(&forge_script_on_mock_bin, perms).expect("Failed to set permissions on mock script");
                    println!("Execute permission set.");
                } else {
                    println!("Execute permission already set.");
                }
            }
            // --- End Add execute permission ---

        } else {
            println!("Mock script file NOT FOUND!");
            // もし見つからなかったら、ls の結果も見てみる
            let ls_output = std::process::Command::new("ls").arg("-al").arg(mock_bin_path.display().to_string()).output();
            match ls_output {
                Ok(out) => println!("ls -al {}:\n{}", mock_bin_path.display(), String::from_utf8_lossy(&out.stdout)),
                Err(e) => println!("Failed to run ls: {}", e),
            }
        }
        // --- End Debug ---

        // canonicalize は存在チェックの後に行う
        let forge_script_path_mock_abs = forge_script_on_mock_bin.canonicalize().expect("Failed to canonicalize mock forge script path");
        let original_path = std::env::var("PATH").unwrap_or_default();
        let mock_bin_abs_path_str = mock_bin_path.canonicalize().unwrap().display().to_string();
        std::env::set_var("PATH", format!("{}:{}", mock_bin_abs_path_str, original_path));

        // --- Debugging: Check which forge is used and run mock script directly ---
        println!("--- Debug Info: test_mc_upgrade_broadcast_success ---");
        let which_output = std::process::Command::new("which").arg("forge").output().expect("Failed to run which forge");
        println!("which forge stdout: {}", String::from_utf8_lossy(&which_output.stdout));
        println!("which forge stderr: {}", String::from_utf8_lossy(&which_output.stderr));
        println!("which forge status: {:?}", which_output.status.code());

        println!("Executing mock script directly: {} script {} --broadcast", forge_script_path_mock_abs.display(), expected_script_path);
        let mock_run_output = std::process::Command::new(forge_script_path_mock_abs)
            .arg("script")
            .arg(expected_script_path)
            .arg("--broadcast")
            .output()
            .expect("Failed to run mock forge script directly");
        println!("Mock script stdout: {}", String::from_utf8_lossy(&mock_run_output.stdout));
        println!("Mock script stderr: {}", String::from_utf8_lossy(&mock_run_output.stderr));
        println!("Mock script status: {:?}", mock_run_output.status.code());
        println!("--- End Debug Info ---");
        // --- End Debugging ---

        let args = McUpgradeArgs { broadcast: Some(true) };
        let result = handler.mc_upgrade(args).await;

        // --- Assertions --- //
        assert!(result.is_ok(), "mc_upgrade broadcast call failed: {:?}", result.err());
        let call_result = result.unwrap();
        // This assertion is currently failing
        assert_eq!(call_result.is_error, Some(false), "Expected success status for upgrade broadcast");
        assert!(!call_result.content.is_empty(), "Expected output content");

        let output_text = &call_result.content[0].raw.as_text().expect("Expected text content").text;
        assert!(output_text.trim_start().starts_with("Forge Upgrade Results:"), "Output should start with upgrade results title");
        assert!(output_text.contains(&format!("Script: {}", expected_script_path)), "Should mention script path");
        assert!(output_text.contains("Broadcast: true"), "Should mention broadcast true");
        assert!(output_text.contains("Transaction Hash: 0xmockhashupgrade"), "Should contain mock upgrade tx hash");

        std::env::set_var("PATH", original_path);
    }

    // Add test for upgrade broadcast failure
    #[tokio::test]
    async fn test_mc_upgrade_broadcast_failure() {
        let manifest_dir_check = env!("CARGO_MANIFEST_DIR");
        std::env::set_current_dir(manifest_dir_check).expect("Failed to set current dir to project root");

        let expected_script_path = "scripts/Upgrade.s.sol";
        // --- Provide valid config for this test case ---
        let config = McpConfig {
            scripts: mc_mcp::config::ScriptsConfig {
                deploy: Some("scripts/Deploy.s.sol".to_string()), // Need some path
                upgrade: Some(expected_script_path.to_string()),
                rpc_url: Some("http://localhost:8545".to_string()),
                private_key_env_var: Some("TEST_PK_ENV".to_string()),
            },
            ..Default::default()
        };
        let (handler, _mock_service) = setup_mock_handler_with_config(config);
        // --- End config setup ---

        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let mock_bin_path = std::path::Path::new(manifest_dir).join("tests/mock_bin");
        if !mock_bin_path.exists() {
            std::fs::create_dir_all(&mock_bin_path).unwrap();
        }
        let forge_script_path_mock = mock_bin_path.join("forge");
        // --- Mock script MUST exit with 1 for failure test ---
        let mock_script_content = format!(
            r#"#!/bin/sh
# This mock should simulate a failed broadcast, so it exits with 1
echo "Mock upgrade broadcast failure called with: $@" >&2 # Log args
if [ "$1" = "script" ] && [ "$2" = "{}" ] && [ "$3" = "--broadcast" ]; then
  echo "Error: Upgrade broadcast failed due to gas estimation." >&2 # Output to stderr
  exit 1 # Exit with error code
else
  echo "Unexpected mock forge call in upgrade broadcast failure test: $@" >&2
  exit 1 # Also exit with error code if args mismatch
fi
"#,
            expected_script_path
        );
        #[cfg(unix)]
        std::fs::write(&forge_script_path_mock, mock_script_content).unwrap();
        #[cfg(windows)] // Basic windows version
        std::fs::write(&forge_script_path_mock, "@echo off\necho Error: Upgrade broadcast failed due to gas estimation. >&2\nexit /b 1").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&forge_script_path_mock).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&forge_script_path_mock, perms).unwrap();
        }

        let original_path = std::env::var("PATH").unwrap_or_default();
        let mock_bin_abs_path = mock_bin_path.canonicalize().unwrap();
        std::env::set_var("PATH", format!("{}:{}", mock_bin_abs_path.display(), original_path));

        let args = McUpgradeArgs { broadcast: Some(true) }; // Broadcast
        let result = handler.mc_upgrade(args).await;

        assert!(result.is_ok());
        let call_result = result.unwrap();
        assert_eq!(call_result.is_error, Some(true), "Expected error status for failed upgrade broadcast");
        assert!(!call_result.content.is_empty());

        let output_text = &call_result.content[0].raw.as_text().expect("Expected text").text;
        assert!(output_text.trim_start().starts_with("Forge Upgrade Results:"), "Output should start with upgrade results title");
        assert!(output_text.contains("Exit Code: 1"), "Should show exit code 1");
        assert!(output_text.contains("Error: Upgrade broadcast failed due to gas estimation."), "Should contain stderr message");

        std::env::set_var("PATH", original_path);
    }

    #[tokio::test]
    async fn test_mc_deploy_broadcast_missing_rpc_url() {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        std::env::set_current_dir(manifest_dir).expect("Failed to set current dir");

        let config = McpConfig {
            scripts: mc_mcp::config::ScriptsConfig {
                deploy: Some("scripts/Deploy.s.sol".to_string()),
                upgrade: Some("scripts/Upgrade.s.sol".to_string()),
                rpc_url: None, // Missing RPC URL
                private_key_env_var: Some("TEST_PK_ENV".to_string()),
            },
            ..Default::default()
        };
        let (handler, _) = setup_mock_handler_with_config(config);

        let args = McDeployArgs { broadcast: Some(true) };
        let result = handler.mc_deploy(args).await;

        assert!(result.is_ok());
        let call_result = result.unwrap();
        assert_eq!(call_result.is_error, Some(true), "Expected error due to missing RPC URL");
        assert!(!call_result.content.is_empty());
        let error_text = &call_result.content[0].raw.as_text().expect("Expected text").text;
        assert!(error_text.contains("requires an RPC URL configured"));
    }

    #[tokio::test]
    async fn test_mc_deploy_broadcast_missing_pk_env() {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        std::env::set_current_dir(manifest_dir).expect("Failed to set current dir");

        let config = McpConfig {
            scripts: mc_mcp::config::ScriptsConfig {
                deploy: Some("scripts/Deploy.s.sol".to_string()),
                upgrade: Some("scripts/Upgrade.s.sol".to_string()),
                rpc_url: Some("http://localhost:8545".to_string()),
                private_key_env_var: None, // Missing PK Env Var
            },
            ..Default::default()
        };
        let (handler, _) = setup_mock_handler_with_config(config);

        let args = McDeployArgs { broadcast: Some(true) };
        let result = handler.mc_deploy(args).await;

        assert!(result.is_ok());
        let call_result = result.unwrap();
        assert_eq!(call_result.is_error, Some(true), "Expected error due to missing PK env var");
        assert!(!call_result.content.is_empty());
        let error_text = &call_result.content[0].raw.as_text().expect("Expected text").text;
        assert!(error_text.contains("requires a private key environment variable name configured"));
    }

}

fn ensure_qdrant_via_docker() -> Result<(), String> {
    // 1. Check if Docker is installed
    let docker_check = std::process::Command::new("docker")
        .arg("--version")
        .output();
    if docker_check.is_err() {
        return Err("Docker is not installed".to_string());
    }

    // 2. Check if Qdrant container is already running
    let ps = std::process::Command::new("docker")
        .args(["ps", "--filter", "name=qdrant", "--format", "{{.Names}}"])
        .output()
        .map_err(|e| format!("Failed to execute docker ps: {e}"))?;
    let ps_stdout = String::from_utf8_lossy(&ps.stdout);
    if ps_stdout.contains("qdrant") {
        eprintln!("✅ Qdrant is already running in Docker.");
    } else {
        // 3. Start Qdrant container
        eprintln!("Qdrant container not found. Starting Qdrant in Docker...");
        let run = std::process::Command::new("docker")
            .args([
                "run",
                "-d",
                "--name",
                "qdrant",
                "-p",
                "6333:6333",
                "-p",
                "6334:6334",
                "qdrant/qdrant",
            ])
            .output()
            .map_err(|e| format!("Failed to execute docker run: {e}"))?;
        if !run.status.success() {
            return Err(format!(
                "Failed to start Qdrant container: {}",
                String::from_utf8_lossy(&run.stderr)
            ));
        }
        eprintln!("Qdrant container started.");
    }

    // 4. Health check (HTTP endpoint retry)
    let endpoint = "http://localhost:6333/collections";
    for i in 1..=5 {
        match ureq::get(endpoint)
            .timeout(std::time::Duration::from_millis(1000))
            .call()
        {
            Ok(resp) if resp.status() == 200 => {
                eprintln!("✅ Qdrant is running and connected!");
                return Ok(());
            }
            _ => {
                eprintln!("Waiting for Qdrant to start... (Retry {i}/5)");
                sleep(Duration::from_secs(2));
            }
        }
    }
    Err("Failed to connect to Qdrant. Check Docker and network settings.".to_string())
}

// Integration tests, moved from src/main.rs
use mc_mcp::{
    config::{McpConfig, ScriptsConfig, DocumentSource, SetupConfig}, // Import necessary config structs
    domain::reference::{ReferenceService, SearchResult, SearchQuery}, // Import domain types
    server::handler::{MyHandler, McDeployArgs, McUpgradeArgs, SearchDocsArgs}, // Import handler and args (assuming SearchDocsArgs moved too or needed by mocks)
};

use rmcp::{
    model::{CallToolResult, Content, RawContent}, // Import MCP types
    Error as McpError,
    serde_json::{self, json}
};

use anyhow::Result;
use log;
use std::sync::{Arc, Mutex};
use futures::future::BoxFuture;
use qdrant_client::qdrant::{PointStruct, ScoredPoint};
use std::env;
use tempfile;


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
    // Keeping get_indexed_sources as it might be useful for some integration tests
    fn get_indexed_sources(&self) -> Vec<DocumentSource> {
        self.indexed_sources.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl ReferenceService for MockReferenceService {
    async fn index_sources(&self, sources: &[DocumentSource]) -> Result<()> {
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
        query: SearchQuery, // Use the imported SearchQuery
        _score_threshold: Option<f32>,
    ) -> Result<Vec<SearchResult>> { // Use anyhow::Result
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

// --- Test Setup Helpers (Copied from main.rs test) --- //
fn setup_mock_handler() -> (MyHandler, Arc<MockReferenceService>) {
    let mock_service = Arc::new(MockReferenceService::default());
    let handler = MyHandler {
        reference_service_state: Arc::new(Mutex::new(Some(mock_service.clone()))),
        config: Arc::new(McpConfig {
            scripts: ScriptsConfig {
                deploy: Some("scripts/Deploy.s.sol".to_string()),
                upgrade: Some("scripts/Upgrade.s.sol".to_string()),
                rpc_url: None,
                private_key_env_var: None,
            },
            ..Default::default()
        }),
    };
    (handler, mock_service)
}

fn setup_mock_handler_with_config(
    config: McpConfig,
) -> (MyHandler, Arc<MockReferenceService>) {
    let mock_service = Arc::new(MockReferenceService::default());
    let handler = MyHandler {
        reference_service_state: Arc::new(Mutex::new(Some(mock_service.clone()))),
        config: Arc::new(config),
    };
    (handler, mock_service)
}

// --- Integration Tests --- //

// Note: Tests related purely to `mc_search_docs_semantic` logic (like checking initialization)
// were moved to `handler.rs` as unit tests.
// The tests remaining here integrate more parts, like forge command execution.

#[tokio::test]
#[ignore] // Keep ignored as it relies on external `forge` command and setup
async fn test_forge_test_tool_runs_successfully() {
    let (handler, _mock_service) = setup_mock_handler();
    // Assume forge command exists and runs without internal errors
    let result = handler.mc_test().await.expect("Tool call should not panic");
    assert_eq!(result.is_error, Some(false));
    assert!(!result.content.is_empty());
    let _ = result.content[0].raw.as_text().expect("Expected text");
}

#[tokio::test]
// This test assumes `forge test` runs but fails internally (non-zero exit)
async fn test_forge_test_tool_reports_failure() {
    let (handler, _mock_service) = setup_mock_handler();
    let result = handler.mc_test().await.expect("Tool call should not panic");
    assert_eq!(result.is_error, Some(true)); // Expect error status
    assert!(!result.content.is_empty());
    let _ = result.content[0].raw.as_text().expect("Expected text");
}

// --- Forge Script Execution Tests (Deploy/Upgrade/Setup) ---

// Helper function for setting up mock forge binary in tests
fn setup_mock_forge(mock_bin_path: &std::path::Path, script_content: &str) {
    if !mock_bin_path.exists() {
        std::fs::create_dir_all(mock_bin_path).unwrap();
    }
    let forge_script_path = mock_bin_path.join("forge");
    std::fs::write(&forge_script_path, script_content).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&forge_script_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&forge_script_path, perms).unwrap();
    }
}

// Helper to manage PATH environment variable for tests
struct PathGuard {
    original_path: String,
}

impl PathGuard {
    fn set(mock_bin_path: &std::path::Path) -> Self {
        let original_path = std::env::var("PATH").unwrap_or_default();
        let mock_bin_abs_path = mock_bin_path.canonicalize().unwrap();
        std::env::set_var("PATH", format!("{}:{}", mock_bin_abs_path.display(), original_path));
        PathGuard { original_path }
    }
}

impl Drop for PathGuard {
    fn drop(&mut self) {
        std::env::set_var("PATH", &self.original_path);
    }
}

// Helper to manage CWD and MC_PROJECT_ROOT
struct ProjectRootGuard {
    original_cwd: std::path::PathBuf,
    original_mc_root: Option<String>,
}

impl ProjectRootGuard {
    fn set(temp_dir_path: &std::path::Path) -> Self {
        let original_cwd = std::env::current_dir().unwrap();
        let original_mc_root = std::env::var("MC_PROJECT_ROOT").ok();
        std::env::set_var("MC_PROJECT_ROOT", temp_dir_path);
        std::env::set_current_dir(temp_dir_path).unwrap();
        ProjectRootGuard { original_cwd, original_mc_root }
    }
}

impl Drop for ProjectRootGuard {
    fn drop(&mut self) {
        std::env::set_current_dir(&self.original_cwd).unwrap();
        if let Some(val) = &self.original_mc_root {
            std::env::set_var("MC_PROJECT_ROOT", val);
        } else {
            std::env::remove_var("MC_PROJECT_ROOT");
        }
    }
}


#[tokio::test]
async fn test_setup_empty_dir() {
    let (handler, _) = setup_mock_handler();
    let temp_dir = tempfile::tempdir().unwrap();
    let _project_root_guard = ProjectRootGuard::set(temp_dir.path()); // Manages CWD and MC_PROJECT_ROOT

    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let mock_bin_path = std::path::Path::new(manifest_dir).join("tests/mock_bin");
    setup_mock_forge(&mock_bin_path, "#!/bin/sh\nexit 0"); // Setup mock forge
    let _path_guard = PathGuard::set(&mock_bin_path); // Manages PATH

    let result = handler.mc_setup().await;
    assert!(result.is_ok());
    let call_result = result.unwrap();
    assert_eq!(call_result.is_error, Some(false), "Expected success");
}

#[tokio::test]
async fn test_setup_non_empty_dir() {
    let (handler, _) = setup_mock_handler();
    let temp_dir = tempfile::tempdir().unwrap();
    std::fs::write(temp_dir.path().join("dummy.txt"), "hello").unwrap();
    let _project_root_guard = ProjectRootGuard::set(temp_dir.path());

    // No need to set up mock forge/PATH as it should fail before calling it

    let result = handler.mc_setup().await;
    assert!(result.is_ok());
    let call_result = result.unwrap();
    assert_eq!(call_result.is_error, Some(true), "Expected error");
    let content_text = &call_result.content[0].raw.as_text().expect("Expected text").text;
    assert!(content_text.contains("The current directory is not empty"));
}

#[tokio::test]
async fn test_setup_force_true_allows_non_empty_dir() {
    let temp_dir = tempfile::tempdir().unwrap();
    std::fs::write(temp_dir.path().join("dummy.txt"), "hello").unwrap();
    let _project_root_guard = ProjectRootGuard::set(temp_dir.path());

    let config = McpConfig {
        setup: SetupConfig { force: true }, // Set force = true
        ..Default::default()
    };
    // Create handler specifically for this test with force=true config
    let handler = MyHandler {
        reference_service_state: Arc::new(Mutex::new(None)), // Mock service state not needed for setup logic itself
        config: Arc::new(config),
    };

    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let mock_bin_path = std::path::Path::new(manifest_dir).join("tests/mock_bin");
    setup_mock_forge(&mock_bin_path, "#!/bin/sh\nexit 0"); // Setup mock forge
    let _path_guard = PathGuard::set(&mock_bin_path); // Manages PATH


    let result = handler.mc_setup().await;
    assert!(result.is_ok());
    let call_result = result.unwrap();
    // Expect success because force=true overrides the non-empty check
    // but the mock forge call succeeds
    assert_eq!(call_result.is_error, Some(false), "Expected success with force=true");
}


#[tokio::test]
async fn test_mc_deploy_dry_run_success() {
    let expected_script_path = "scripts/Deploy.s.sol";
    let (handler, _mock_service) = setup_mock_handler(); // Uses default deploy path

    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let mock_bin_path = std::path::Path::new(manifest_dir).join("tests/mock_bin");
    let mock_script_content = format!(
        r#"#!/bin/sh
if [ "$1" = "script" ] && [ "$2" = "{}" ] && [ "$#" -eq 2 ]; then
echo "Deploy Dry run successful for {}"
exit 0
else
echo "Error: Unexpected args in deploy dry run: $@" >&2
exit 1
fi
"#,
        expected_script_path, expected_script_path
    );
    setup_mock_forge(&mock_bin_path, &mock_script_content);
    let _path_guard = PathGuard::set(&mock_bin_path);

    let args = McDeployArgs { broadcast: Some(false) };
    let result = handler.mc_deploy(args).await;

    assert!(result.is_ok(), "mc_deploy call failed: {:?}", result.err());
    let call_result = result.unwrap();
    assert_eq!(call_result.is_error, Some(false), "Expected success status");
    let output_text = &call_result.content[0].raw.as_text().expect("Expected text").text;
    assert!(output_text.contains("Deploy Dry Run Results"));
    assert!(output_text.contains(expected_script_path));
}

#[tokio::test]
async fn test_mc_deploy_broadcast_success() {
    let expected_script_path = "scripts/Deploy.s.sol";
    let expected_rpc_url = "http://localhost:8545";
    let expected_pk_env = "TEST_PRIVATE_KEY";
    let config = McpConfig {
        scripts: ScriptsConfig {
            deploy: Some(expected_script_path.to_string()),
            upgrade: Some("scripts/Upgrade.s.sol".to_string()),
            rpc_url: Some(expected_rpc_url.to_string()),
            private_key_env_var: Some(expected_pk_env.to_string()),
        },
        ..Default::default()
    };
    let (handler, _mock_service) = setup_mock_handler_with_config(config);

    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let mock_bin_path = std::path::Path::new(manifest_dir).join("tests/mock_bin");
    let mock_script_content = format!(
        r#"#!/bin/sh
# Check arguments for broadcast
if [ "$1" = "script" ] && [ "$2" = "{}" ] && [ "$3" = "--broadcast" ] && \
   [ "$4" = "--rpc-url" ] && [ "$5" = "{}" ] && \
   [ "$6" = "--private-key" ] && [ "$7" = "\${}" ] && [ "$#" -eq 7 ]; then
echo "Deploy Broadcast successful"
echo "TxHash: 0x123abc"
exit 0
else
echo "Error: Unexpected args in deploy broadcast: $@" >&2
exit 1
fi
"#,
        expected_script_path, expected_rpc_url, expected_pk_env
    );
    setup_mock_forge(&mock_bin_path, &mock_script_content);
    let _path_guard = PathGuard::set(&mock_bin_path);

    let args = McDeployArgs { broadcast: Some(true) };
    let result = handler.mc_deploy(args).await;

    assert!(result.is_ok(), "mc_deploy call failed: {:?}", result.err());
    let call_result = result.unwrap();
    assert_eq!(call_result.is_error, Some(false), "Expected success status. Output: {:?}", call_result.content);
    let output_text = &call_result.content[0].raw.as_text().expect("Expected text").text;
    assert!(output_text.contains("Forge Deploy Results"));
    assert!(output_text.contains("Broadcast: true"));
    assert!(output_text.contains("Exit Code: 0"));
    assert!(output_text.contains("TxHash: 0x123abc"));
}

#[tokio::test]
async fn test_mc_deploy_broadcast_failure() {
    let expected_script_path = "scripts/Deploy.s.sol";
    let config = McpConfig {
        scripts: ScriptsConfig {
            deploy: Some(expected_script_path.to_string()),
            upgrade: Some("scripts/Upgrade.s.sol".to_string()),
            rpc_url: Some("http://localhost:8545".to_string()),
            private_key_env_var: Some("TEST_PK_ENV".to_string()),
        },
        ..Default::default()
    };
    let (handler, _mock_service) = setup_mock_handler_with_config(config);

    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let mock_bin_path = std::path::Path::new(manifest_dir).join("tests/mock_bin");
    let mock_script_content = format!(
        r#"#!/bin/sh
# Simulate failure
echo "Error: Deploy broadcast failed simulation." >&2
exit 1
"#,
    );
    setup_mock_forge(&mock_bin_path, &mock_script_content);
    let _path_guard = PathGuard::set(&mock_bin_path);

    let args = McDeployArgs { broadcast: Some(true) };
    let result = handler.mc_deploy(args).await;

    assert!(result.is_ok());
    let call_result = result.unwrap();
    assert_eq!(call_result.is_error, Some(true), "Expected error status");
    let output_text = &call_result.content[0].raw.as_text().expect("Expected text").text;
    assert!(output_text.contains("Forge Deploy Results"));
    assert!(output_text.contains("Exit Code: 1"));
    assert!(output_text.contains("Error: Deploy broadcast failed simulation."));
}

#[tokio::test]
async fn test_mc_deploy_broadcast_missing_rpc_url() {
    let config = McpConfig {
        scripts: ScriptsConfig {
            deploy: Some("scripts/Deploy.s.sol".to_string()),
            upgrade: Some("scripts/Upgrade.s.sol".to_string()),
            rpc_url: None, // Missing RPC
            private_key_env_var: Some("TEST_PK_ENV".to_string()),
        },
        ..Default::default()
    };
    let (handler, _) = setup_mock_handler_with_config(config);

    let args = McDeployArgs { broadcast: Some(true) };
    let result = handler.mc_deploy(args).await;

    assert!(result.is_ok());
    let call_result = result.unwrap();
    assert_eq!(call_result.is_error, Some(true));
    let error_text = &call_result.content[0].raw.as_text().expect("Expected text").text;
    assert!(error_text.contains("requires an RPC URL configured"));
}

#[tokio::test]
async fn test_mc_deploy_broadcast_missing_pk_env() {
    let config = McpConfig {
        scripts: ScriptsConfig {
            deploy: Some("scripts/Deploy.s.sol".to_string()),
            upgrade: Some("scripts/Upgrade.s.sol".to_string()),
            rpc_url: Some("http://localhost:8545".to_string()),
            private_key_env_var: None, // Missing PK env
        },
        ..Default::default()
    };
    let (handler, _) = setup_mock_handler_with_config(config);

    let args = McDeployArgs { broadcast: Some(true) };
    let result = handler.mc_deploy(args).await;

    assert!(result.is_ok());
    let call_result = result.unwrap();
    assert_eq!(call_result.is_error, Some(true));
    let error_text = &call_result.content[0].raw.as_text().expect("Expected text").text;
    assert!(error_text.contains("requires a private key environment variable name configured"));
}


// --- mc_upgrade Tests --- //

#[tokio::test]
async fn test_mc_upgrade_dry_run_success() {
    let expected_script_path = "scripts/Upgrade.s.sol";
    let (handler, _mock_service) = setup_mock_handler(); // Uses default upgrade path

    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let mock_bin_path = std::path::Path::new(manifest_dir).join("tests/mock_bin");
    let mock_script_content = format!(
        r#"#!/bin/sh
if [ "$1" = "script" ] && [ "$2" = "{}" ] && [ "$#" -eq 2 ]; then
echo "Upgrade Dry run successful for {}"
exit 0
else
echo "Error: Unexpected args in upgrade dry run: $@" >&2
exit 1
fi
"#,
        expected_script_path, expected_script_path
    );
    setup_mock_forge(&mock_bin_path, &mock_script_content);
    let _path_guard = PathGuard::set(&mock_bin_path);

    let args = McUpgradeArgs { broadcast: Some(false) };
    let result = handler.mc_upgrade(args).await;

    assert!(result.is_ok(), "mc_upgrade call failed: {:?}", result.err());
    let call_result = result.unwrap();
    assert_eq!(call_result.is_error, Some(false), "Expected success status");
    let output_text = &call_result.content[0].raw.as_text().expect("Expected text").text;
    assert!(output_text.contains("Upgrade Dry Run Results"));
    assert!(output_text.contains(expected_script_path));
}

#[tokio::test]
async fn test_mc_upgrade_no_script_configured() {
    let config = McpConfig {
        scripts: ScriptsConfig {
            deploy: Some("scripts/Deploy.s.sol".to_string()),
            upgrade: None, // No upgrade script
            rpc_url: None,
            private_key_env_var: None,
        },
        ..Default::default()
    };
    let (handler, _mock_service) = setup_mock_handler_with_config(config);

    let args = McUpgradeArgs { broadcast: Some(false) };
    let result = handler.mc_upgrade(args).await;

    assert!(result.is_ok());
    let call_result = result.unwrap();
    assert_eq!(call_result.is_error, Some(true));
    let error_text = &call_result.content[0].raw.as_text().expect("Expected text").text;
    assert!(error_text.contains("Upgrade script path is not configured"));
}

#[tokio::test]
async fn test_mc_upgrade_dry_run_failure() {
    let expected_script_path = "scripts/Upgrade.s.sol";
    let (handler, _mock_service) = setup_mock_handler();

    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let mock_bin_path = std::path::Path::new(manifest_dir).join("tests/mock_bin");
    let mock_script_content = format!(
        r#"#!/bin/sh
# Simulate failure
echo "Error: Upgrade dry run failed simulation." >&2
exit 1
"#,
    );
    setup_mock_forge(&mock_bin_path, &mock_script_content);
    let _path_guard = PathGuard::set(&mock_bin_path);

    let args = McUpgradeArgs { broadcast: Some(false) };
    let result = handler.mc_upgrade(args).await;

    assert!(result.is_ok());
    let call_result = result.unwrap();
    assert_eq!(call_result.is_error, Some(true), "Expected error status");
    let output_text = &call_result.content[0].raw.as_text().expect("Expected text").text;
    assert!(output_text.contains("Forge Upgrade Dry Run Results"));
    assert!(output_text.contains("Exit Code: 1"));
    assert!(output_text.contains("Error: Upgrade dry run failed simulation."));
}

#[tokio::test]
async fn test_mc_upgrade_broadcast_success() {
    let expected_script_path = "scripts/Upgrade.s.sol";
    let expected_rpc_url = "http://localhost:8545";
    let expected_pk_env = "TEST_PRIVATE_KEY_UPGRADE";
    let config = McpConfig {
        scripts: ScriptsConfig {
            deploy: Some("scripts/Deploy.s.sol".to_string()),
            upgrade: Some(expected_script_path.to_string()),
            rpc_url: Some(expected_rpc_url.to_string()),
            private_key_env_var: Some(expected_pk_env.to_string()),
        },
        ..Default::default()
    };
    let (handler, _mock_service) = setup_mock_handler_with_config(config);

    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let mock_bin_path = std::path::Path::new(manifest_dir).join("tests/mock_bin");
    let mock_script_content = format!(
        r#"#!/bin/sh
# Check arguments for broadcast
if [ "$1" = "script" ] && [ "$2" = "{}" ] && [ "$3" = "--broadcast" ] && \
   [ "$4" = "--rpc-url" ] && [ "$5" = "{}" ] && \
   [ "$6" = "--private-key" ] && [ "$7" = "\${}" ] && [ "$#" -eq 7 ]; then
echo "Upgrade Broadcast successful"
echo "TxHash: 0x456def"
exit 0
else
echo "Error: Unexpected args in upgrade broadcast: $@" >&2
exit 1
fi
"#,
        expected_script_path, expected_rpc_url, expected_pk_env
    );
    setup_mock_forge(&mock_bin_path, &mock_script_content);
    let _path_guard = PathGuard::set(&mock_bin_path);

    let args = McUpgradeArgs { broadcast: Some(true) };
    let result = handler.mc_upgrade(args).await;

    assert!(result.is_ok(), "mc_upgrade call failed: {:?}", result.err());
    let call_result = result.unwrap();
    assert_eq!(call_result.is_error, Some(false), "Expected success status. Output: {:?}", call_result.content);
    let output_text = &call_result.content[0].raw.as_text().expect("Expected text").text;
    assert!(output_text.contains("Forge Upgrade Results"));
    assert!(output_text.contains("Broadcast: true"));
    assert!(output_text.contains("Exit Code: 0"));
    assert!(output_text.contains("TxHash: 0x456def"));
}

#[tokio::test]
async fn test_mc_upgrade_broadcast_failure() {
    let expected_script_path = "scripts/Upgrade.s.sol";
    let config = McpConfig {
        scripts: ScriptsConfig {
            deploy: Some("scripts/Deploy.s.sol".to_string()),
            upgrade: Some(expected_script_path.to_string()),
            rpc_url: Some("http://localhost:8545".to_string()),
            private_key_env_var: Some("TEST_PK_ENV".to_string()),
        },
        ..Default::default()
    };
    let (handler, _mock_service) = setup_mock_handler_with_config(config);

    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let mock_bin_path = std::path::Path::new(manifest_dir).join("tests/mock_bin");
    let mock_script_content = format!(
        r#"#!/bin/sh
# Simulate failure
echo "Error: Upgrade broadcast failed simulation." >&2
exit 1
"#,
    );
    setup_mock_forge(&mock_bin_path, &mock_script_content);
    let _path_guard = PathGuard::set(&mock_bin_path);

    let args = McUpgradeArgs { broadcast: Some(true) };
    let result = handler.mc_upgrade(args).await;

    assert!(result.is_ok());
    let call_result = result.unwrap();
    assert_eq!(call_result.is_error, Some(true), "Expected error status");
    let output_text = &call_result.content[0].raw.as_text().expect("Expected text").text;
    assert!(output_text.contains("Forge Upgrade Results"));
    assert!(output_text.contains("Exit Code: 1"));
    assert!(output_text.contains("Error: Upgrade broadcast failed simulation."));
}

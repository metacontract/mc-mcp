use log;
use std::process::Command;
use std::time::Duration;

// Function to check and start Qdrant using Docker, wrapped for async execution.
pub async fn ensure_qdrant_via_docker() -> Result<(), String> {
    tokio::task::spawn_blocking(|| {
        ensure_qdrant_sync()
    }).await.map_err(|e| format!("Failed to execute blocking task: {}", e))?
}

// Original synchronous implementation
fn ensure_qdrant_sync() -> Result<(), String> {
    const QDRANT_CONTAINER_NAME: &str = "mc-mcp-qdrant";
    // 1. Check if Docker is installed
    let docker_check = Command::new("docker")
        .arg("--version")
        .output();
    if docker_check.is_err() {
        log::error!("Docker check failed. Is Docker installed and running?");
        return Err("Docker is not installed or not runnable.".to_string());
    }
    log::info!("Docker found.");

    // 2. Check if Qdrant container already exists (any state)
    log::info!("Checking for Qdrant container: {}", QDRANT_CONTAINER_NAME);
    let ps = Command::new("docker")
        .args(["ps", "-a", "--filter", &format!("name={}", QDRANT_CONTAINER_NAME), "--format", "{{.Status}}"])
        .output()
        .map_err(|e| format!("Failed to execute docker ps: {e}"))?;

    if !ps.status.success() {
        log::error!("docker ps command failed: {}", String::from_utf8_lossy(&ps.stderr));
        return Err(format!("Failed to check container status: {}", String::from_utf8_lossy(&ps.stderr)));
    }

    let ps_stdout = String::from_utf8_lossy(&ps.stdout);
    log::debug!("docker ps output: {}", ps_stdout);

    if ps_stdout.contains("Up") {
        log::info!("✅ Qdrant container '{}' is already running.", QDRANT_CONTAINER_NAME);
    } else if !ps_stdout.trim().is_empty() {
        // Exists but not running (e.g. Exited)
        log::info!("Qdrant container '{}' exists but is not running. Starting it...", QDRANT_CONTAINER_NAME);
        let start = Command::new("docker")
            .args(["start", QDRANT_CONTAINER_NAME])
            .output()
            .map_err(|e| format!("Failed to execute docker start: {e}"))?;
        if !start.status.success() {
            log::error!("Failed to start Qdrant container '{}': {}", QDRANT_CONTAINER_NAME, String::from_utf8_lossy(&start.stderr));
            return Err(format!(
                "Failed to start existing Qdrant container '{}': {}",
                QDRANT_CONTAINER_NAME,
                String::from_utf8_lossy(&start.stderr)
            ));
        }
        log::info!("Qdrant container '{}' started.", QDRANT_CONTAINER_NAME);
    } else {
        // Not found, run new container
        log::info!("Qdrant container '{}' not found. Creating and starting new container...", QDRANT_CONTAINER_NAME);
        let run = Command::new("docker")
            .args([
                "run",
                "-d",
                "--name",
                QDRANT_CONTAINER_NAME,
                "-p",
                "6333:6333",
                "-p",
                "6334:6334",
                "qdrant/qdrant",
            ])
            .output()
            .map_err(|e| format!("Failed to execute docker run: {e}"))?;
        if !run.status.success() {
            log::error!("Failed to run Qdrant container '{}': {}", QDRANT_CONTAINER_NAME, String::from_utf8_lossy(&run.stderr));
            return Err(format!(
                "Failed to start new Qdrant container '{}': {}",
                QDRANT_CONTAINER_NAME,
                String::from_utf8_lossy(&run.stderr)
            ));
        }
        log::info!("Qdrant container '{}' created and started.", QDRANT_CONTAINER_NAME);
    }

    // 3. Health check (HTTP endpoint retry)
    log::info!("Performing health check on Qdrant...");
    let endpoint = "http://localhost:6333/collections";
    for i in 1..=10 { // Increased retries
        log::debug!("Health check attempt {}/10 for {}", i, endpoint);
        match ureq::get(endpoint)
            .timeout(Duration::from_secs(2))
            .call()
        {
            Ok(resp) if resp.status() == 200 => {
                log::info!("✅ Qdrant is running and connected!");
                return Ok(());
            }
            Ok(resp) => {
                log::warn!("Qdrant health check returned status: {}. Waiting... (Retry {}/10)", resp.status(), i);
                std::thread::sleep(Duration::from_secs(3)); // Increased sleep time
            }
            Err(e) => {
                log::warn!("Qdrant health check connection error: {}. Waiting... (Retry {}/10)", e, i);
                std::thread::sleep(Duration::from_secs(3)); // Increased sleep time
            }
        }
    }
    log::error!("Failed to connect to Qdrant after multiple retries.");
    Err("Failed to connect to Qdrant after retries. Check Docker container logs and network settings.".to_string())
}

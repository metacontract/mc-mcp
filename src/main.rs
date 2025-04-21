use rmcp::ServiceExt;
use std::sync::Arc;
use tokio::{
    io::{stdin, stdout},
    sync::Notify,
};

use mc_mcp::config;
use mc_mcp::initialization::initialize_background_services;
use mc_mcp::server::handler::McpServerHandler;

use anyhow::Result;
use env_logger;
use log;

#[tokio::main]
async fn main() -> Result<()> {
    // --- Basic Setup First ---
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("trace"))
        .target(env_logger::Target::Stderr)
        .init();
    log::info!("mc-mcp server (MCP over stdio) started.");

    // --- Load Configuration ---
    let config = config::load_config()?;
    log::info!("Configuration loaded: {:?}", config);
    let config_arc = Arc::new(config);

    // --- Initialize Handler ---
    let handler = McpServerHandler::new(config_arc.clone());
    let reference_service_state = handler.reference_service_state.clone(); // for background init

    // --- Start MCP Server Immediately ---
    let transport = (stdin(), stdout());
    log::info!("Starting MCP server listener...");
    let serve_future = handler.serve(transport);

    // --- Start Background Initialization Task (using initialization module) ---
    let init_config = config_arc.clone(); // Keep config clone
    let initialization_complete = Arc::new(Notify::new()); // Keep notify
    let init_complete_signal = initialization_complete.clone(); // Keep notify clone

    tokio::spawn(async move {
        log::info!("Background initialization task started.");
        // Use the function from the initialization module
        let init_result = initialize_background_services(init_config, reference_service_state).await;

        match init_result {
            Ok(_) => {
                log::info!("Background initialization completed successfully.");
                init_complete_signal.notify_one();
            }
            Err(e) => {
                log::error!("Background initialization failed: {}", e);
                // Optionally notify waiters about the failure state.
            }
        }
    });

    log::info!("MCP server started, waiting for initialization and client connection...");

    // Now await the server serving future
    let server_handle = serve_future.await.inspect_err(|e| {
        log::error!("serving error: {:?}", e);
    })?;

    log::info!("mc-mcp server running, waiting for completion...");
    let shutdown_reason = server_handle.waiting().await?;
    log::info!("mc-mcp server finished. Reason: {:?}", shutdown_reason);

    Ok(())
}

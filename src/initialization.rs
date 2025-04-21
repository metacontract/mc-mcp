use crate::application::reference_service::ReferenceServiceImpl;
use crate::config::{self, McpConfig}; // Import config and McpConfig
use crate::domain::reference::ReferenceService;
use crate::domain::vector_repository::VectorRepository;
use crate::file_system;
use crate::infrastructure::embedding::EmbeddingGenerator;
use crate::infrastructure::file_system::download_if_not_exists;
use crate::infrastructure::vector_db::{qdrant_client, VectorDb};
use crate::infrastructure::EmbeddingModel;
use crate::infrastructure::docker::ensure_qdrant_via_docker; // Import from new location

use anyhow::Result;
use log;
use std::sync::{Arc, Mutex};

const PREBUILT_INDEX_URL: &str =
    "https://github.com/metacontract/mc-mcp/releases/latest/download/prebuilt_index.jsonl.gz";

/// Performs all the heavy initialization in the background.
pub async fn initialize_background_services(
    config_arc: Arc<McpConfig>,
    service_state: Arc<Mutex<Option<Arc<dyn ReferenceService>>>>,
) -> Result<()> {
    // --- Ensure Qdrant is Running ---
    if let Err(e) = ensure_qdrant_via_docker().await { // Now awaitable if ensure_qdrant becomes async
        log::error!("Qdrant check/start failed: {}", e);
        // Consider returning error depending on requirements
        // return Err(anyhow::anyhow!("Failed to ensure Qdrant: {}", e));
    }

    // --- Download Prebuilt Index (If Configured and Not Exists) ---
    if let Some(dest_path_buf) = &config_arc.reference.prebuilt_index_path {
        log::info!("Checking/Downloading prebuilt index to {:?}...", dest_path_buf);
        if let Some(dest_str) = dest_path_buf.to_str() {
            if let Some(parent_dir) = dest_path_buf.parent() {
                if !parent_dir.exists() {
                    tokio::fs::create_dir_all(parent_dir).await?;
                }
            }
            let download_result = tokio::task::spawn_blocking({
                let url = PREBUILT_INDEX_URL.to_string();
                let dest = dest_str.to_string();
                move || download_if_not_exists(&url, &dest)
            }).await?;

            match download_result {
                Ok(_) => log::info!("Checked/Downloaded prebuilt index to {:?}", dest_path_buf),
                Err(e) => log::error!("Failed check/download prebuilt index: {}", e),
            }
        } else {
            log::error!("Invalid UTF-8 prebuilt index path: {:?}", dest_path_buf);
        }
    } else {
        log::info!("Skipping prebuilt index download check (no path configured).");
    }

    // --- Download Docs Archive (If Configured and Not Exists) ---
    if let Some(docs_path_buf) = &config_arc.reference.docs_archive_path {
         log::info!("Checking/Downloading docs archive to {:?}...", docs_path_buf);
         if let Some(docs_str) = docs_path_buf.to_str() {
             if let Some(parent_dir) = docs_path_buf.parent() {
                 if !parent_dir.exists() {
                    tokio::fs::create_dir_all(parent_dir).await?;
                 }
             }
             // Consider making get_latest_release_download_url async or running in spawn_blocking
             let docs_archive_url = config::get_latest_release_download_url(
                 config::GITHUB_REPO_OWNER,
                 config::GITHUB_REPO_NAME,
                 config::DOCS_ARCHIVE_FILENAME,
             );
             let download_result = tokio::task::spawn_blocking({
                 let url = docs_archive_url.clone();
                 let dest = docs_str.to_string();
                 move || download_if_not_exists(&url, &dest)
             }).await?;

             match download_result {
                 Ok(_) => log::info!("Checked/Downloaded docs archive to {:?}", docs_path_buf),
                 Err(e) => log::error!("Failed check/download docs archive: {}", e),
             }
         } else {
             log::error!("Invalid UTF-8 docs archive path: {:?}", docs_path_buf);
         }
    } else {
         log::info!("Skipping docs archive download check (no path configured).");
    }

    // --- Dependency Injection Setup ---
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
    let embedding_cache_dir = config_arc.reference.embedding_cache_dir.clone();

    let embedder_result = tokio::task::spawn_blocking(move || EmbeddingGenerator::new(embedding_model, embedding_cache_dir)).await?;

    let embedder = match embedder_result {
        Ok(generator) => Arc::new(generator),
        Err(e) => {
            log::error!("Failed to create EmbeddingGenerator: {:?}", e);
            return Err(e);
        }
    };

    let vector_db_instance =
        VectorDb::new(Box::new(qdrant_client), collection_name.clone(), vector_dim)?;
    vector_db_instance.initialize_collection().await?;
    let vector_db: Arc<dyn VectorRepository> = Arc::new(vector_db_instance);

    // --- Prebuilt Index Loading (If configured) ---
    if let Some(prebuilt_path) = config_arc.reference.prebuilt_index_path.clone() {
        log::info!("Attempting to load prebuilt index from: {:?}", prebuilt_path);
        match tokio::task::spawn_blocking(move || file_system::load_prebuilt_index(prebuilt_path)).await? {
            Ok(prebuilt_docs) => {
                log::info!("Loaded {} docs from prebuilt index.", prebuilt_docs.len());
                if !prebuilt_docs.is_empty() {
                    log::info!("Upserting prebuilt documents...");
                    match vector_db.upsert_documents(&prebuilt_docs).await {
                        Ok(_) => log::info!("Successfully upserted prebuilt documents."),
                        Err(e) => log::error!("Failed to upsert prebuilt documents: {}", e),
                    }
                }
            }
            Err(e) => {
                log::error!("Failed to load prebuilt index: {}. Continuing...", e);
            }
        }
    } else {
        log::info!("No prebuilt index path configured.");
    }

    // --- Initialize Reference Service ---
    let reference_service = Arc::new(ReferenceServiceImpl::new(embedder, vector_db.clone(), config_arc.clone()));

    // --- Initial Indexing from configured sources ---
    log::info!("Triggering indexing from configured sources...");
    if let Err(e) = reference_service
        .index_sources(&config_arc.reference.sources)
        .await
    {
        log::error!("Error during configured source indexing: {}", e);
        // Consider propagating error: return Err(e.into());
    }
    log::info!("Configured source indexing process started/completed.");

    // --- Update Handler State with Initialized Service ---
    {
        let mut state = service_state.lock().unwrap();
        *state = Some(reference_service);
        log::info!("ReferenceService is now initialized and available.");
    }

    Ok(())
}

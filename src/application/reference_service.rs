// NOTE: use statements adjusted for single-crate structure
use crate::domain::reference::{SearchQuery, SearchResult};
use crate::infrastructure::{
    EmbeddingGenerator,
    VectorDb,
    DocumentToUpsert,
    DocumentPayload,
    load_documents,
    EmbeddingModel,
};
use crate::infrastructure::qdrant_client::qdrant::{
    point_id::PointIdOptions,
    value::Kind as QdrantValueKind,
};
use std::path::PathBuf;
use std::sync::Arc;
use anyhow::{anyhow, Result};
use log::{info, error, warn};
use serde_json::Value;
// Remove Qdrant direct use if only used via VectorDb re-export
// use qdrant_client::Qdrant;

// Assuming VectorRepository trait exists in domain
use crate::domain::vector_repository::VectorRepository;
use crate::config::{ReferenceConfig, DocumentSource, SourceType};
use crate::infrastructure::file_system::{load_documents_from_multiple_sources, load_documents_from_source};

// Define the interface for reference-related operations
#[async_trait::async_trait]
pub trait ReferenceService: Send + Sync {
    async fn index_documents(&self, docs_path: Option<PathBuf>) -> Result<()>;
    async fn index_sources(&self, sources: &[DocumentSource]) -> Result<()>;
    async fn search_documents(&self, query: SearchQuery, score_threshold: Option<f32>) -> Result<Vec<SearchResult>>;
}

// Implementation using infrastructure components
pub struct ReferenceServiceImpl {
    embedder: Arc<EmbeddingGenerator>,
    // Inject the trait object for VectorDb
    vector_db: Arc<dyn VectorRepository>, // Use trait object
    // Configuration might be needed, e.g., chunk size, model name
}

impl ReferenceServiceImpl {
    // Consider adding configuration struct later
    pub fn new(embedder: Arc<EmbeddingGenerator>, vector_db: Arc<dyn VectorRepository>) -> Self {
        Self { embedder, vector_db }
    }

    // Simple chunking logic (example: split by paragraphs or fixed size)
    fn chunk_document(&self, file_path: &str, content: &str) -> Vec<String> {
        // Placeholder: Split by double newline (paragraph)
        // A more robust solution would handle different markdown structures better
        // and potentially use a sliding window or size-based chunking.
        log::debug!("Chunking document: {}", file_path);
        content.split("\n\n")
               .map(str::trim)
               .filter(|s| !s.is_empty())
               .map(String::from)
               .collect()
    }

    // Helper to process and upsert chunks from a single source
    async fn process_and_upsert_source(
        &self,
        source_name: &str,
        documents: &std::collections::HashMap<String, String>
    ) -> Result<()> {
        let mut all_docs_to_upsert: Vec<DocumentToUpsert> = Vec::new();
        let mut total_chunks = 0;

        for (file_path, content) in documents {
            let chunks = self.chunk_document(file_path, content);
            total_chunks += chunks.len();
            log::debug!("Generated {} chunks for {}", chunks.len(), file_path);

            if chunks.is_empty() {
                continue;
            }

            let chunk_slices: Vec<&str> = chunks.iter().map(AsRef::as_ref).collect();
            let embeddings = match self.embedder.generate_embeddings(&chunk_slices) {
                Ok(e) => e,
                Err(e) => {
                    error!("Failed to generate embeddings for {}: {}", file_path, e);
                    continue; // Skip this file if embedding fails
                }
            };
            log::debug!("Generated {} embeddings for {}", embeddings.len(), file_path);

            let docs_to_upsert: Vec<DocumentToUpsert> = chunks.into_iter()
                .zip(embeddings.into_iter())
                .map(|(chunk_content, vector)| {
                    // Create DocumentToUpsert with all necessary fields
                    DocumentToUpsert {
                        file_path: file_path.clone(),
                        vector,
                        source: Some(source_name.to_string()), // Pass the source name as Option
                        content_chunk: chunk_content, // Include the actual text chunk
                        metadata: None, // Set metadata to None for now
                    }
                })
                .collect();

            all_docs_to_upsert.extend(docs_to_upsert);
        }

        log::info!("Generated {} chunks for source '{}'.", total_chunks, source_name);

        if !all_docs_to_upsert.is_empty() {
            log::info!("Upserting {} chunks for source '{}'...", all_docs_to_upsert.len(), source_name);
            // Call the repository method
            match self.vector_db.upsert_documents(&all_docs_to_upsert).await {
                Ok(_) => log::info!("Upsert completed for source '{}'.", source_name),
                Err(e) => {
                    // Log the error but potentially continue with other sources
                    error!("Upsert failed for source '{}': {}", source_name, e);
                    // Depending on desired behavior, you might return the error here
                    // return Err(e); // Uncomment to stop on first upsert error
                }
            }
        } else {
            log::warn!("No chunks generated for source '{}'.", source_name);
        }

        Ok(())
    }
}

#[async_trait::async_trait]
impl ReferenceService for ReferenceServiceImpl {
    async fn index_documents(&self, docs_path: Option<PathBuf>) -> Result<()> {
        warn!("index_documents is deprecated; use index_sources instead.");
        if let Some(path) = docs_path {
            let source = DocumentSource {
                name: "default_mc_docs".to_string(), // Assign a default name
                source_type: SourceType::Local,
                path,
            };
            self.index_sources(&[source]).await
        } else {
            error!("index_documents called without a path; cannot proceed.");
            Err(anyhow!("index_documents requires a path when called directly"))
        }
    }

    async fn index_sources(&self, sources: &[DocumentSource]) -> Result<()> {
        log::info!("Starting indexing for {} configured sources...", sources.len());
        let mut had_error = false;

        for source in sources {
            log::info!("Processing source: '{}' ({:?}: {})", source.name, source.source_type, source.path.display());

            match source.source_type {
                SourceType::Local => {
                    // TODO: Replace with actual loading logic from infrastructure
                    // This function needs to be created/adapted in infrastructure::file_system
                    match load_documents_from_source(&source.path) { // Pass only path for now
                        Ok(documents) => {
                            if let Err(e) = self.process_and_upsert_source(&source.name, &documents).await {
                                error!("Error processing source '{}': {}", source.name, e);
                                had_error = true;
                            }
                        }
                        Err(e) => {
                            error!("Error loading documents for source '{}': {}", source.name, e);
                            had_error = true;
                        }
                    }
                }
                // Add other source types (Git, Http) later
                // _ => {
                //     warn!("Source type {:?} for '{}' is not yet supported.", source.source_type, source.name);
                // }
            }
        }

        if had_error {
            Err(anyhow!("One or more errors occurred during indexing. See logs for details."))
        } else {
            log::info!("Finished processing all configured sources.");
            Ok(())
        }
    }

    async fn search_documents(&self, query: SearchQuery, score_threshold: Option<f32>) -> Result<Vec<SearchResult>> {
        log::info!("Performing search for query: '{}', limit: {:?}", query.text, query.limit);

        // 1. Generate embedding for the query
        let query_embedding = self.embedder.generate_embeddings(&[&query.text])?
            .pop()
            .ok_or_else(|| anyhow!("Failed to generate embedding for query: {}", query.text))?;

        // 2. Search using VectorDb repository (already returns Vec<SearchResult>)
        let search_limit = query.limit.unwrap_or(5); // Default limit
        match self.vector_db.search(query_embedding, search_limit, score_threshold).await {
            Ok(results) => {
                log::info!("Search returned {} results from repository.", results.len());
                Ok(results)
            }
            Err(e) => {
                error!("Search failed in repository: {}", e);
                // Propagate the error
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use std::path::PathBuf;
    use std::fs::{self, File};
    use std::io::Write;
    use std::sync::Arc;
    use crate::infrastructure::EmbeddingModel;
    use crate::infrastructure::EmbeddingGenerator;
    use crate::infrastructure::vector_db::VectorDb;
    use crate::domain::vector_repository::VectorRepository;
    use anyhow::Result;

    // 仮のReferenceConfig構造体（本実装時はconfig.rs等に分離）
    #[derive(Debug, Clone)]
    struct ReferenceSourceConfig {
        pub path: PathBuf,
        pub source: String,
    }
    #[derive(Debug, Clone)]
    struct ReferenceConfig {
        pub sources: Vec<ReferenceSourceConfig>,
    }

    #[tokio::test]
    async fn test_index_documents_with_additional_sources_and_config() -> Result<()> {
        // --- テスト用ディレクトリ・ファイル作成 ---
        let main_dir = tempdir()?;
        let main_md = main_dir.path().join("main.md");
        let mut f1 = File::create(&main_md)?;
        writeln!(f1, "# Main doc")?;
        drop(f1);

        let add1 = tempdir()?;
        let add1_md = add1.path().join("add1.md");
        let mut f2 = File::create(&add1_md)?;
        writeln!(f2, "# Add1 doc")?;
        drop(f2);

        let add2 = tempdir()?;
        let add2_md = add2.path().join("add2.md");
        let mut f3 = File::create(&add2_md)?;
        writeln!(f3, "# Add2 doc")?;
        drop(f3);

        // --- 仮のReferenceConfigを構築 ---
        let config = ReferenceConfig {
            sources: vec![
                ReferenceSourceConfig { path: main_dir.path().to_path_buf(), source: "mc-docs".to_string() },
                ReferenceSourceConfig { path: add1.path().to_path_buf(), source: "additional-1".to_string() },
                ReferenceSourceConfig { path: add2.path().to_path_buf(), source: "additional-2".to_string() },
            ],
        };

        // --- EmbeddingGenerator, VectorDbのモック/スタブを用意（本実装時はテスト用Qdrantを使う） ---
        // ここではダミーの実装やモックを使う想定（詳細はGreenフェーズで実装）
        // let embedder = Arc::new(EmbeddingGenerator::new(EmbeddingModel::AllMiniLML6V2)?);
        // let vector_db = Arc::new(MockVectorDb::new());
        // let service = ReferenceServiceImpl::new(embedder, vector_db);

        // --- テスト本体: config.sourcesを使って全てのドキュメントがインデックス化されることを検証 ---
        // 1. config.sourcesをload_documents_from_multiple_sourcesに渡してドキュメントをロード
        // 2. chunk→embedding→upsertの流れを通す
        // 3. VectorDbに正しいsourceメタデータ付きで保存されることを検証
        // ※ここではRedフェーズなので、未実装部分はtodo!()やコメントでOK

        // 仮実装: load_documents_from_multiple_sourcesの呼び出し例
        let sources: Vec<(PathBuf, String)> = config.sources.iter().map(|s| (s.path.clone(), s.source.clone())).collect();
        let doc_map = crate::infrastructure::file_system::load_documents_from_multiple_sources(&sources)
            .expect("Should load all sources");
        assert_eq!(doc_map.len(), 3, "全てのドキュメントがインデックス化されること");
        assert!(doc_map.values().any(|(_text, src)| src == "mc-docs"));
        assert!(doc_map.values().any(|(_text, src)| src == "additional-1"));
        assert!(doc_map.values().any(|(_text, src)| src == "additional-2"));

        // --- 以降はGreenフェーズでchunk→embedding→upsertの流れを検証 ---

        Ok(())
    }
}

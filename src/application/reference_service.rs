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

// Define the interface for reference-related operations
#[async_trait::async_trait]
pub trait ReferenceService: Send + Sync {
    async fn index_documents(&self, docs_path: Option<PathBuf>) -> Result<()>;
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
}

#[async_trait::async_trait]
impl ReferenceService for ReferenceServiceImpl {
    async fn index_documents(&self, docs_path: Option<PathBuf>) -> Result<()> {
        log::info!("Starting document indexing from path: {:?}...", docs_path);

        // 1. Load documents (using infrastructure function)
        let document_map = crate::infrastructure::load_documents(docs_path.clone())
            .map_err(|e| anyhow!("Failed to load documents from {:?}: {}", docs_path, e))?;

        if document_map.is_empty() {
            log::warn!("No documents found to index in path: {:?}", docs_path);
            return Ok(());
        }
        log::info!("Loaded {} documents.", document_map.len());

        let mut all_docs_to_upsert: Vec<DocumentToUpsert> = Vec::new();
        let mut total_chunks = 0;

        // 2. Chunk, Embed, and Prepare for Upsert
        for (file_path, content) in &document_map {
            let chunks = self.chunk_document(file_path, content);
            total_chunks += chunks.len();
            log::debug!("Generated {} chunks for {}", chunks.len(), file_path);

            if chunks.is_empty() {
                continue;
            }

            // Convert Vec<String> to Vec<&str> for embedding
            let chunk_slices: Vec<&str> = chunks.iter().map(AsRef::as_ref).collect();

            // 3. Generate embeddings
            let embeddings = self.embedder.generate_embeddings(&chunk_slices)?;
            log::debug!("Generated {} embeddings for {}", embeddings.len(), file_path);

            // 4. Prepare DocumentToUpsert structs
            let docs_to_upsert: Vec<DocumentToUpsert> = chunks.into_iter()
                .zip(embeddings.into_iter())
                .map(|(_chunk_content, vector)| { // We don't need chunk_content here, just the path and vector
                    DocumentToUpsert {
                        file_path: file_path.clone(), // Associate chunk with original file path
                        vector,
                        // text_content: chunk_content, // Could store chunk text in payload if needed
                    }
                })
                .collect();

            all_docs_to_upsert.extend(docs_to_upsert);
        }

        log::info!("Generated a total of {} chunks from {} documents.", total_chunks, document_map.len());

        // 5. Upsert documents into Vector DB (using injected trait object)
        if !all_docs_to_upsert.is_empty() {
            log::info!("Upserting {} document chunks into the vector database...", all_docs_to_upsert.len());
            // Corrected: Call upsert_documents on the trait object. Assuming VectorRepository has this method.
            // If VectorRepository only has upsert(single), we need to loop or add upsert_documents to the trait.
            // Let's assume upsert_documents exists on the trait for now.
             self.vector_db.upsert_documents(&all_docs_to_upsert).await?;
            log::info!("Document indexing completed successfully.");
        } else {
            log::warn!("No document chunks were generated or prepared for upserting.");
        }

        Ok(())
    }

    async fn search_documents(&self, query: SearchQuery, score_threshold: Option<f32>) -> Result<Vec<SearchResult>> {
        log::info!("Performing search for query: '{}', limit: {}", query.text, query.limit.unwrap_or(5));

        // 1. Generate embedding for the query
        let query_embedding = self.embedder.generate_embeddings(&[&query.text])?
            .pop() // Get the first (and only) embedding
            .ok_or_else(|| anyhow!("Failed to generate embedding for query: {}", query.text))?;

        // 2. Search using VectorDb (via injected trait object)
        let search_limit = query.limit.unwrap_or(5); // Default limit
        // Corrected: Call search on the trait object. Assuming VectorRepository::search returns Vec<ScoredPoint>.
        // The current VectorRepository impl in vector_db.rs returns Vec<DocumentToUpsert>.
        // This needs alignment. Let's assume the trait method returns ScoredPoint for now, matching the old code.
        let search_results = self.vector_db.search(query_embedding, search_limit, score_threshold).await?;

        // 3. Convert ScoredPoint results to domain::SearchResult
        let domain_results: Vec<SearchResult> = search_results.into_iter()
            .filter_map(|scored_point| {
                 // Assuming ScoredPoint structure from qdrant_client
                use crate::infrastructure::qdrant_client::qdrant::point_id::PointIdOptions;
                use crate::infrastructure::qdrant_client::qdrant::value::Kind as QdrantValueKind;
                use crate::infrastructure::DocumentPayload; // Use DocumentPayload from infra
                use serde_json::Value;

                let point_id_clone = scored_point.id.clone(); // Clone for potential use in error messages
                let document_id_str = match point_id_clone.map(|id| id.point_id_options).flatten() {
                     Some(PointIdOptions::Uuid(uuid_str)) => uuid_str,
                     Some(PointIdOptions::Num(num)) => num.to_string(),
                     None => "<unknown_id>".to_string(),
                 };

                if !scored_point.payload.is_empty() {
                    let payload_map = scored_point.payload.clone();
                    let mut json_map = serde_json::Map::new();

                    for (key, value) in payload_map {
                        let json_value = match value.kind {
                             Some(QdrantValueKind::NullValue(_)) => serde_json::Value::Null,
                             Some(QdrantValueKind::BoolValue(b)) => serde_json::Value::Bool(b),
                             Some(QdrantValueKind::DoubleValue(d)) => {
                                 serde_json::Number::from_f64(d)
                                     .map(serde_json::Value::Number)
                                     .unwrap_or_else(|| {
                                         warn!("Could not convert non-finite f64 to JSON number: {} for key '{}' in point ID {}", d, key, document_id_str);
                                         serde_json::Value::Null
                                     })
                             }
                             Some(QdrantValueKind::IntegerValue(i)) => serde_json::Value::Number(i.into()),
                             Some(QdrantValueKind::StringValue(s)) => serde_json::Value::String(s),
                             Some(QdrantValueKind::ListValue(_)) | Some(QdrantValueKind::StructValue(_)) => {
                                 warn!("Unsupported Qdrant value kind (List/Struct) for key '{}' in point ID {}", key, document_id_str);
                                 serde_json::Value::Null
                             }
                             None => serde_json::Value::Null,
                         };
                         json_map.insert(key, json_value);
                    }

                    let intermediate_json_value = Value::Object(json_map);

                    match serde_json::from_value::<DocumentPayload>(intermediate_json_value.clone()) {
                        Ok(payload) => Some(SearchResult {
                            file_path: payload.file_path,
                            score: scored_point.score,
                        }),
                        Err(e) => {
                            error!("Failed to deserialize DocumentPayload from converted JSON for point ID {}: {}. JSON was: {}", document_id_str, e, intermediate_json_value);
                            None
                        }
                    }
                } else {
                    info!("Point {} has no payload, skipping.", document_id_str);
                    None
                }
            })
            .collect();

        log::info!("Search returned {} results.", domain_results.len());
        Ok(domain_results)
    }
}

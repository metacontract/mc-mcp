use mc_mcp_domain::reference::{SearchQuery, SearchResult};
use mc_mcp_infrastructure::{
    EmbeddingGenerator,
    VectorDb,
    DocumentToUpsert,
    DocumentPayload,
    load_documents,
};
use qdrant_client::qdrant::{
    point_id::PointIdOptions,
    value::Kind as QdrantValueKind,
};
use std::path::PathBuf;
use std::sync::Arc;
use anyhow::{anyhow, Result};
use log::{info, error, warn};
use serde_json::Value;

// Define the interface for reference-related operations
#[async_trait::async_trait]
pub trait ReferenceService: Send + Sync {
    async fn index_documents(&self, docs_path: Option<PathBuf>) -> Result<()>;
    async fn search_documents(&self, query: SearchQuery) -> Result<Vec<SearchResult>>;
}

// Implementation using infrastructure components
pub struct ReferenceServiceImpl {
    embedder: Arc<EmbeddingGenerator>,
    vector_db: Arc<VectorDb>,
    // Configuration might be needed, e.g., chunk size, model name
}

impl ReferenceServiceImpl {
    // Consider adding configuration struct later
    pub fn new(embedder: Arc<EmbeddingGenerator>, vector_db: Arc<VectorDb>) -> Self {
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

        // 1. Load documents
        let document_map = load_documents(docs_path.clone())
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

        // 5. Upsert documents into Vector DB
        if !all_docs_to_upsert.is_empty() {
            log::info!("Upserting {} document chunks into the vector database...", all_docs_to_upsert.len());
            self.vector_db.upsert_documents(&all_docs_to_upsert).await?;
            log::info!("Document indexing completed successfully.");
        } else {
            log::warn!("No document chunks were generated or prepared for upserting.");
        }

        Ok(())
    }

    async fn search_documents(&self, query: SearchQuery) -> Result<Vec<SearchResult>> {
        log::info!("Performing search for query: '{}', limit: {}", query.text, query.limit.unwrap_or(5));

        // 1. Generate embedding for the query
        // Note: generate_embeddings expects a slice, so pass a slice with one element
        let query_embedding = self.embedder.generate_embeddings(&[&query.text])?
            .pop() // Get the first (and only) embedding
            .ok_or_else(|| anyhow!("Failed to generate embedding for query: {}", query.text))?;

        // 2. Search using VectorDb
        let search_limit = query.limit.unwrap_or(5); // Default limit
        let search_results = self.vector_db.search(query_embedding, search_limit).await?;

        // 3. Convert ScoredPoint results to domain::SearchResult
        let domain_results: Vec<SearchResult> = search_results.into_iter()
            .filter_map(|scored_point| {
                // Clone scored_point.id before using `?` to avoid moving the value.
                let document_id = match scored_point.id.clone()?.point_id_options? {
                    PointIdOptions::Uuid(uuid_str) => uuid_str,
                    PointIdOptions::Num(num) => num.to_string(), // Consider how to handle u64 IDs if necessary
                };

                // Convert Qdrant payload (HashMap<String, QdrantValue>) to serde_json::Value::Object
                // Check if payload is not empty before processing
                if !scored_point.payload.is_empty() {
                    let payload_map = scored_point.payload.clone(); // Clone needed here as scored_point is used later for id/score
                    let mut json_map = serde_json::Map::new();

                    for (key, value) in payload_map {
                        // Directly convert QdrantValueKind to serde_json::Value
                        let json_value = match value.kind {
                            Some(QdrantValueKind::NullValue(_)) => serde_json::Value::Null,
                            Some(QdrantValueKind::BoolValue(b)) => serde_json::Value::Bool(b),
                            Some(QdrantValueKind::DoubleValue(d)) => {
                                // Safely convert f64 to JSON Number, handling non-finite values
                                serde_json::Number::from_f64(d)
                                    .map(serde_json::Value::Number)
                                    .unwrap_or_else(|| {
                                        warn!("Could not convert non-finite f64 to JSON number: {} for key '{}' in point ID {:?}", d, key, scored_point.id);
                                        serde_json::Value::Null // Convert non-finite numbers to Null
                                    })
                            }
                            Some(QdrantValueKind::IntegerValue(i)) => serde_json::Value::Number(i.into()),
                            Some(QdrantValueKind::StringValue(s)) => serde_json::Value::String(s),
                            Some(QdrantValueKind::ListValue(_)) | Some(QdrantValueKind::StructValue(_)) => {
                                // Log unsupported types and convert to Null
                                warn!("Unsupported Qdrant value kind (List/Struct) for key '{}' in point ID {:?}", key, scored_point.id);
                                serde_json::Value::Null
                            }
                            None => serde_json::Value::Null, // Handle case where value.kind is None
                        };
                        json_map.insert(key, json_value);
                    }

                    // Convert the constructed serde_json::Map to serde_json::Value
                    let intermediate_json_value = Value::Object(json_map);

                    // Attempt to deserialize into DocumentPayload
                    match serde_json::from_value::<DocumentPayload>(intermediate_json_value.clone()) { // Clone intermediate value
                        Ok(payload) => {
                            // Need to extract file_path from DocumentPayload, not SearchResult fields directly
                            // Prefix with underscore as it seems unused after assignment. Re-evaluate if it's needed.
                            let _point_id_str = match scored_point.id.clone().and_then(|id| id.point_id_options) {
                                Some(PointIdOptions::Uuid(s)) => s,
                                Some(PointIdOptions::Num(n)) => n.to_string(),
                                None => "<unknown_id>".to_string(),
                            };

                            Some(SearchResult {
                                // file_path field name needs adjustment based on SearchResult definition
                                // Use payload.file_path or whatever is correct
                                file_path: payload.file_path, // Corrected: Use DocumentPayload's file_path
                                score: scored_point.score,
                            })
                        },
                        Err(e) => {
                            // Prefix with underscore as it seems unused after assignment.
                            let _point_id_str = match scored_point.id.clone().and_then(|id| id.point_id_options) {
                                Some(PointIdOptions::Uuid(s)) => s,
                                Some(PointIdOptions::Num(n)) => n.to_string(),
                                None => "<unknown_id>".to_string(),
                            };
                            error!("Failed to deserialize DocumentPayload from converted JSON for point ID {}: {}. JSON was: {}", document_id, e, intermediate_json_value); // Use document_id captured earlier
                            None
                        }
                    }
                } else {
                    // Log or handle cases where payload is empty or not requested
                    // Prefix with underscore as it seems unused after assignment.
                    let _point_id_str = match scored_point.id.clone().and_then(|id| id.point_id_options) {
                        Some(PointIdOptions::Uuid(s)) => s,
                        Some(PointIdOptions::Num(n)) => n.to_string(),
                        None => "<unknown_id>".to_string(),
                    };
                    info!("Point {} has no payload, skipping.", document_id); // Use document_id captured earlier
                    None
                }
            })
            .collect();

        log::info!("Search returned {} results.", domain_results.len());
        Ok(domain_results)
    }
}

// Basic tests for the service implementation structure
#[cfg(test)]
mod tests {
    use super::*;
    use mc_mcp_domain::reference::{SearchQuery, SearchResult};
    use mc_mcp_infrastructure::{EmbeddingModel, VectorDb, Qdrant, EmbeddingGenerator, DocumentToUpsert, DocumentPayload};
    use std::sync::Arc;
    use tempfile::tempdir;
    use std::fs::File;
    use std::io::Write;
    use uuid; // Import uuid crate for test collection name
    use testcontainers::{core::{IntoContainerPort, WaitFor}, runners::AsyncRunner, GenericImage}; // Import testcontainers items
    use simple_logger; // Import simple_logger


    // Mock implementations or use testcontainers for integration tests
    // For now, just test the structure and basic logic flow (requires mocks or further setup)

    #[tokio::test]
    async fn test_reference_service_impl_structure() {
        // This test requires proper setup of mock/real embedder and vector_db
        // Skipping actual execution for now, just checking structure compiles

        // Example setup (replace with actual or mocks)
        // let embedder = Arc::new(EmbeddingGenerator::new(EmbeddingModel::BGESmallENV15).expect("Failed to init embedder"));
        // Needs a running Qdrant instance or mock client
        // let qdrant_client = Qdrant::from_url("http://localhost:6334").build().expect("Qdrant client failed");
        // let vector_db = Arc::new(VectorDb::new(qdrant_client, "test_collection".to_string(), 384).unwrap());

        // let service = ReferenceServiceImpl::new(embedder, vector_db);

        assert!(true); // Placeholder assertion
        println!("Skipping actual execution in test_reference_service_impl_structure - requires mock/integration setup.");
    }

     // Add more tests, including integration tests for index_documents and search_documents
     // using testcontainers for Qdrant and potentially mock embedder or real one.

    // Helper function to setup Qdrant container (copied and adapted from infrastructure tests)
    async fn setup_qdrant_for_app_test() -> Result<(testcontainers::ContainerAsync<testcontainers::GenericImage>, String)> {
        use testcontainers::{core::{IntoContainerPort, WaitFor}, runners::AsyncRunner, GenericImage};
        const QDRANT_IMAGE_NAME: &str = "qdrant/qdrant";
        const QDRANT_IMAGE_TAG: &str = "latest";
        const QDRANT_GRPC_PORT: u16 = 6334;

        log::info!("Starting Qdrant container for app integration test...");
        let image = GenericImage::new(QDRANT_IMAGE_NAME, QDRANT_IMAGE_TAG)
            .with_exposed_port(QDRANT_GRPC_PORT.tcp())
            .with_wait_for(WaitFor::message_on_stderr("Qdrant initialization completed"));

        let container = image.start().await?;
        let host_port = container.get_host_port_ipv4(QDRANT_GRPC_PORT).await?;
        let qdrant_url = format!("http://localhost:{}", host_port);
        log::info!("Qdrant container started, gRPC accessible at: {}", qdrant_url);
        Ok((container, qdrant_url))
    }

    // Example Integration Test (requires testcontainers setup similar to infrastructure tests)
    #[tokio::test]
    #[ignore] // Ignore by default as it requires a running Qdrant via testcontainers and network access for model download
    async fn test_integration_index_and_search() -> Result<()> {
        simple_logger::SimpleLogger::new().init().unwrap_or(()); // Initialize logger for tests

        // 1. Setup testcontainer for Qdrant
        let (_container, qdrant_url) = setup_qdrant_for_app_test().await?;
         let qdrant_client = Qdrant::from_url(&qdrant_url).build()?;
         let collection_name = format!("test_app_{}", uuid::Uuid::new_v4());
         let vector_dim = 384; // Match embedder model (BGESmallENV15)

        // 2. Create temporary directory and test markdown file
        let dir = tempdir()?;
        let docs_path = dir.path().to_path_buf();
        let file_path = docs_path.join("test_doc.md");
        let mut file = File::create(&file_path)?;
        writeln!(file, "# Document Title\n\nThis is the first paragraph.\n\nThis second paragraph contains important keywords for testing.")?;
        drop(file); // Ensure file is closed


        // 3. Setup Embedder and VectorDb
        let embedder = Arc::new(EmbeddingGenerator::new(EmbeddingModel::BGESmallENV15)?);
        let vector_db = Arc::new(VectorDb::new(qdrant_client.clone(), collection_name.clone(), vector_dim)?);
        vector_db.initialize_collection().await?; // Initialize the collection

        // 4. Create Service Instance
        let service = ReferenceServiceImpl::new(embedder.clone(), vector_db.clone());

        // 5. Index the document
        log::info!("Starting indexing for integration test...");
        service.index_documents(Some(docs_path)).await?;

        // Optional: Verify points exist in Qdrant
        tokio::time::sleep(std::time::Duration::from_millis(500)).await; // Give time for indexing
        log::info!("Checking point count after indexing...");
        let count_response = qdrant_client.count_points(&collection_name, None, Some(true)).await?;
        let count = count_response.result.ok_or_else(|| anyhow!("Count response missing result"))?.count;
        assert!(count > 0, "No points were indexed (count={})", count);
        log::info!("Found {} points after indexing.", count);


        // 6. Search for content
        log::info!("Performing search...");
        let query = SearchQuery {
            text: "important keywords".to_string(), // Query text targeting the second paragraph
            limit: Some(1),
            // filter: None,
        };
        let results = service.search_documents(query).await?;

        // 7. Assert results
        assert_eq!(results.len(), 1, "Should find one matching document chunk");
        let top_result = &results[0];
        log::info!("Top search result: {:?}", top_result);
        assert_eq!(top_result.file_path, file_path.to_string_lossy().to_string(), "Found document path mismatch");
        assert!(top_result.score > 0.5, "Score should be reasonably high for relevant query (got {})", top_result.score); // Adjust threshold as needed
        // Further assertions on fragment if implemented

        dir.close()?; // Clean up temp directory
        Ok(())
    }
}

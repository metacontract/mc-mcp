use mc_mcp_domain::reference::{SearchQuery, SearchResult};
use mc_mcp_infrastructure::{
    EmbeddingGenerator,
    VectorDb,
    DocumentToUpsert,
    DocumentPayload,
    load_documents,
    EmbeddingModel,
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
use qdrant_client::Qdrant;
use testcontainers::{core::WaitFor, Docker, GenericContainer};
use qdrant_client::prelude::*;
use std::collections::HashMap;
use uuid::Uuid;

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
    use mc_mcp_domain::reference::SearchQuery;
    use mc_mcp_infrastructure::{EmbeddingModel, VectorDb, EmbeddingGenerator};
    use qdrant_client::Qdrant;
    use std::sync::Arc;
    use tempfile::tempdir;
    use std::fs::File;
    use std::io::Write;
    use testcontainers::{core::WaitFor, Docker, GenericContainer};
    use qdrant_client::prelude::*;
    use std::collections::HashMap;

    const QDRANT_IMAGE: &str = "qdrant/qdrant";
    const QDRANT_TAG: &str = "latest";
    const QDRANT_GRPC_PORT: u16 = 6334;
    const EMBEDDING_DIM: u64 = 768;

    fn qdrant_container() -> GenericContainer {
        let env_vars = HashMap::new();
        GenericContainer::new(QDRANT_IMAGE, QDRANT_TAG)
            .with_exposed_port(QDRANT_GRPC_PORT)
            .with_wait_for(WaitFor::message_on_stderr("Qdrant startup finished"))
            .with_env_vars(env_vars)
    }

    #[tokio::test]
    async fn test_qdrant_generic_connection() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let docker = clients::Cli::default();
        let qdrant_node = docker.run(qdrant_container());
        let grpc_port = qdrant_node.get_host_port_ipv4(QDRANT_GRPC_PORT);
        let grpc_uri = format!("http://localhost:{}", grpc_port);
        println!("Qdrant gRPC (Generic) listening on: {}", grpc_uri);

        let client = Qdrant::from_url(&grpc_uri).build()?;

        let collections_list = client.list_collections().await?;
        println!("Collections: {:?}", collections_list);
        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn test_integration_index_and_search() -> Result<()> {
        let docker = clients::Cli::default();
        let qdrant_node = docker.run(qdrant_container());
        let grpc_port = qdrant_node.get_host_port_ipv4(QDRANT_GRPC_PORT);
        let grpc_uri = format!("http://localhost:{}", grpc_port);
        info!("Qdrant for integration test listening on: {}", grpc_uri);

        let qdrant_client = Qdrant::from_url(&grpc_uri).build()?;

        let collection_name = format!("test_integration_{}", Uuid::new_v4());
        info!("Using collection name: {}", collection_name);

        let create_collection_request = CreateCollection {
            collection_name: collection_name.clone(),
            vectors_config: Some(qdrant_client::qdrant::VectorsConfig {
                config: Some(qdrant_client::qdrant::vectors_config::Config::Params(
                    VectorParams {
                        size: EMBEDDING_DIM,
                        distance: Distance::Cosine.into(),
                        hnsw_config: None,
                        quantization_config: None,
                        multivector_config: None,
                        datatype: None,
                    }
                ))
            }),
            ..Default::default()
        };
        let response: CollectionOperationResponse = qdrant_client.create_collection(create_collection_request).await?;
        if !response.result {
             return Err(anyhow!("Failed to create Qdrant collection '{}': Time={}", collection_name, response.time));
        }
        info!("Successfully created collection '{}'", collection_name);

        let vector_db = VectorDb::new(qdrant_client.clone(), collection_name.clone(), EMBEDDING_DIM)?;
        let vector_db_arc = Arc::new(vector_db);

        let embedder = EmbeddingGenerator::new(Some(EmbeddingModel::Default))?;
        let embedder_arc = Arc::new(embedder);

        let reference_service = ReferenceServiceImpl::new(embedder_arc.clone(), vector_db_arc.clone());

        let temp_dir = tempdir()?;
        let docs_path = temp_dir.path().to_path_buf();
        let dummy_md_path = docs_path.join("test_doc.md");
        let mut file = File::create(&dummy_md_path)?;
        writeln!(file, "# Test Document\n\nThis is the first paragraph.\n\nThis is the second paragraph.")?;
        drop(file);

        reference_service.index_documents(Some(docs_path)).await?;
        info!("Indexing completed for test.");

        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        let count_request = CountPoints {
            collection_name: collection_name.clone(),
            filter: None,
            exact: Some(true),
        };
        let count_response = qdrant_client.count(count_request).await?;
        info!("Count after indexing: {:?}", count_response);
        let point_count = count_response.result.map_or(0, |c| c.count);
        assert!(point_count > 0, "Expected points were not found after indexing. Count: {}", point_count);
        assert_eq!(point_count, 2, "Expected 2 chunks based on markdown. Found: {}", point_count);

        let search_query = SearchQuery {
            text: "second paragraph".to_string(),
            limit: Some(1),
        };
        let search_results = reference_service.search_documents(search_query).await?;
        info!("Search results: {:?}", search_results);

        assert!(!search_results.is_empty(), "Search returned no results");
        assert_eq!(search_results.len(), 1, "Search should return 1 result");
        assert!(search_results[0].file_path.ends_with("test_doc.md"), "Result file path mismatch");
        assert!(search_results[0].score > 0.0, "Search result score should be positive");

        info!("Integration test completed successfully.");
        Ok(())
    }
}

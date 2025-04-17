// NOTE: use statements will need adjustment after moving files
use serde::{Deserialize, Serialize};
use log;
// Use the re-exported module path for Qdrant internally
pub use qdrant_client; // Re-export the entire module
use self::qdrant_client::Qdrant;
use self::qdrant_client::{
    Payload,
    qdrant::{PointStruct, ScoredPoint, SearchPoints, PointId, Vectors, WithPayloadSelector, WithVectorsSelector, Distance, VectorParams, CreateCollectionBuilder, UpsertPointsBuilder},
};
use uuid::Uuid;
use anyhow::{anyhow, Result};
use async_trait::async_trait;

// Assuming DocumentToUpsert is now in embedding.rs
use super::embedding::DocumentToUpsert;
// Assuming VectorRepository trait is in domain/vector_repository.rs
use crate::domain::vector_repository::VectorRepository;
use self::qdrant_client::qdrant::value::Kind as QdrantValueKind;
use crate::domain::reference::SearchResult; // Import domain SearchResult

mod payload {
    use serde::{Deserialize, Serialize};
    #[derive(Serialize, Deserialize, Debug, Clone)]
    pub struct DocumentPayload {
        pub file_path: String,
        pub source: Option<String>, // Make optional to match SearchResult
        pub content_chunk: String, // Add the chunk content
        pub metadata: Option<serde_json::Value>, // Add optional metadata
    }
}
pub use self::payload::DocumentPayload;

// --- Document to Upsert --- (Consider moving this definition)
// Structure passed to upsert_documents. Contains data needed to create PointStruct.
#[derive(Debug, Clone)]
pub struct DocumentToUpsert {
    pub file_path: String,
    pub vector: Vec<f32>,
    pub source: Option<String>, // Make optional
    pub content_chunk: String, // Add content chunk
    pub metadata: Option<serde_json::Value>, // Add metadata
}

pub struct VectorDb {
    client: Box<Qdrant>,
    collection_name: String,
    vector_size: u64,
}

impl VectorDb {
    /// Creates a new VectorDb instance.
    ///
    /// # Arguments
    ///
    /// * `client` - An initialized Qdrant client (the new Qdrant struct).
    /// * `collection_name` - The name of the collection to use.
    /// * `vector_size` - The dimension of the vectors.
    ///
    /// # Returns
    ///
    /// A Result containing the `VectorDb` instance on success.
    pub fn new(client: Box<Qdrant>, collection_name: String, vector_size: u64) -> Result<Self> {
        // Basic validation
        if collection_name.is_empty() {
            return Err(anyhow!("Collection name cannot be empty"));
        }
        if vector_size == 0 {
            return Err(anyhow!("Vector size must be greater than zero"));
        }
        Ok(Self { client, collection_name, vector_size })
    }

     /// Initializes the Qdrant collection if it doesn't exist.
    ///
    /// # Returns
    ///
    /// A Result indicating success or failure.
    pub async fn initialize_collection(&self) -> Result<()> {
        log::info!("Checking if collection '{}' exists...", self.collection_name);

        match self.client.collection_info(&self.collection_name).await {
            Ok(_) => {
                log::info!("Collection '{}' already exists.", self.collection_name);
                Ok(())
            }
            Err(e) => {
                 // Log the specific error for debugging
                log::warn!("Collection '{}' not found or error checking existence: {}. Attempting to create...", self.collection_name, e);
                 // Check if the error indicates "Not Found" specifically if the client library supports it
                 // Qdrant client might return a specific status code or error kind for "Not Found"
                 // Assuming any error means we should try to create it for now.
                 self.create_collection_internal().await
            }
        }
    }

    // Internal helper to create the collection
    async fn create_collection_internal(&self) -> Result<()> {
        log::info!("Creating collection '{}' with size {} and distance Cosine...", self.collection_name, self.vector_size);

        // Use the builder pattern with struct literal for VectorParams
        let vector_params = VectorParams {
            size: self.vector_size,
            distance: Distance::Cosine.into(),
            hnsw_config: None,
            quantization_config: None,
            on_disk: None,
            multivector_config: None,
            datatype: None,
        };
        // Pass vector_params directly, removing .into()
        let create_builder = CreateCollectionBuilder::new(self.collection_name.clone())
            .vectors_config(vector_params);
            // Add other builder methods if needed

        match self.client.create_collection(create_builder).await {
            Ok(_) => {
                log::info!("Successfully created collection '{}'.", self.collection_name);
                Ok(())
            }
            Err(e) => {
                log::error!("Failed to create collection '{}': {}", self.collection_name, e);
                Err(anyhow!("Failed to create collection: {}", e))
            }
        }
    }

    /// Upserts (inserts or updates) documents into the Qdrant collection.
    pub async fn upsert_documents_impl(&self, documents: &[DocumentToUpsert]) -> Result<()> {
        if documents.is_empty() {
            log::info!("No documents provided for upsert.");
            return Ok(());
        }

        log::info!("Preparing to upsert {} documents into collection '{}'...", documents.len(), self.collection_name);

        let points: Vec<PointStruct> = documents
            .iter()
            .filter_map(|doc| { // Use filter_map to skip points with errors
                // Create the payload struct with all necessary fields
                let payload_struct = DocumentPayload {
                    file_path: doc.file_path.clone(),
                    source: doc.source.clone(),
                    content_chunk: doc.content_chunk.clone(),
                    metadata: doc.metadata.clone(),
                };
                // Serialize the payload struct to a serde_json::Value first
                let payload_value: serde_json::Value = match serde_json::to_value(payload_struct) {
                    Ok(v) => v,
                    Err(e) => {
                        log::error!("Failed to serialize DocumentPayload for file '{}': {}", doc.file_path, e);
                        return None; // Skip this point
                    }
                };
                // Try converting the serde_json::Value to Qdrant Payload
                let payload: Payload = match Payload::try_from(payload_value) {
                     Ok(p) => p,
                     Err(e) => {
                         log::error!("Failed to convert serde_json::Value to Qdrant Payload for file '{}': {}", doc.file_path, e);
                         return None; // Skip this point
                     }
                 };

                // Create unique ID for each chunk
                let point_id: PointId = PointId::from(Uuid::new_v4().to_string());

                Some(PointStruct {
                    id: Some(point_id),
                    vectors: Some(Vectors::from(doc.vector.clone())),
                    payload: payload.into(), // Use the converted Payload
                })
            })
            .collect();

        if points.is_empty() {
             log::warn!("No valid points could be prepared for upserting (input count: {}). Check serialization/conversion errors.", documents.len());
             // Return Ok as no *upsert* operation failed, just preparation
             return Ok(());
        }

        let points_count = points.len();
        log::info!("Upserting {} valid points into collection '{}'...", points_count, self.collection_name);

        let upsert_builder = UpsertPointsBuilder::new(self.collection_name.clone(), points)
             .wait(true);

        match self.client.upsert_points(upsert_builder).await {
            Ok(response) => {
                log::debug!("Upsert response: {:?}", response);
                if let Some(result) = response.result {
                     log::info!("Upsert operation completed with status: {:?}", result.status());
                } else {
                    log::warn!("Upsert response did not contain result details.");
                }
                // Use the stored count
                log::info!("Successfully requested upsert for {} points.", points_count);
                Ok(())
            }
            Err(e) => {
                log::error!("Failed to upsert points into collection '{}': {}", self.collection_name, e);
                Err(anyhow!("Qdrant upsert failed: {}", e))
            }
        }
    }

    /// Searches for documents in the Qdrant collection and returns domain SearchResult.
    pub async fn search_impl(&self, query_vector: Vec<f32>, limit: usize, score_threshold: Option<f32>) -> Result<Vec<SearchResult>> {
         if query_vector.len() as u64 != self.vector_size {
            return Err(anyhow!(
                "Query vector dimension ({}) does not match collection dimension ({})",
                query_vector.len(),
                self.vector_size
            ));
        }

        log::info!("Searching in collection '{}' with limit {}...", self.collection_name, limit);

        let search_request = SearchPoints {
            collection_name: self.collection_name.clone(),
            vector: query_vector,
            limit: limit as u64,
            with_payload: Some(WithPayloadSelector { // Request payload
                 selector_options: Some(qdrant_client::qdrant::with_payload_selector::SelectorOptions::Enable(true)),
            }),
            with_vectors: Some(WithVectorsSelector{ // Don't need vectors in result
                 selector_options: Some(qdrant_client::qdrant::with_vectors_selector::SelectorOptions::Enable(false)),
            }),
            score_threshold,
            ..Default::default()
        };

        log::debug!("Sending search request: {:?}", search_request);

        match self.client.search_points(search_request).await {
            Ok(response) => {
                log::info!("Search completed successfully, found {} potential results.", response.result.len());

                // Map ScoredPoint to domain::SearchResult
                let search_results: Vec<SearchResult> = response.result.into_iter()
                    .filter_map(|scored_point| {
                        // Extract payload
                        let payload_map = scored_point.payload;
                        if payload_map.is_empty() {
                            log::warn!("Search result point {:?} has no payload, skipping.", scored_point.id);
                            return None;
                        }

                        // Convert Qdrant Payload map back to serde_json::Value
                        // This helper function might be useful
                        let json_value = Self::qdrant_payload_to_serde_value(payload_map)?;

                        // Deserialize into our DocumentPayload struct
                        match serde_json::from_value::<DocumentPayload>(json_value) {
                            Ok(payload_data) => Some(SearchResult {
                                file_path: payload_data.file_path,
                                score: scored_point.score,
                                source: payload_data.source,
                                content_chunk: payload_data.content_chunk,
                                metadata: payload_data.metadata,
                            }),
                            Err(e) => {
                                log::error!("Failed to deserialize DocumentPayload from search result {:?}: {}", scored_point.id, e);
                                None
                            }
                        }
                    })
                    .collect();

                log::info!("Successfully mapped {} results to SearchResult.", search_results.len());
                Ok(search_results)
            }
            Err(e) => {
                 log::error!("Qdrant search failed in collection '{}': {}", self.collection_name, e);
                Err(anyhow!("Qdrant search failed: {}", e))
            }
        }
    }

    // Helper function to convert Qdrant Payload map to serde_json::Value
    // Returns Option<Value> because conversion might fail for a point
    fn qdrant_payload_to_serde_value(payload_map: std::collections::HashMap<String, qdrant_client::qdrant::Value>) -> Option<serde_json::Value> {
        let mut json_map = serde_json::Map::new();
        for (key, qdrant_value) in payload_map {
            let json_value = match qdrant_value.kind {
                Some(QdrantValueKind::NullValue(_)) => serde_json::Value::Null,
                Some(QdrantValueKind::BoolValue(b)) => serde_json::Value::Bool(b),
                Some(QdrantValueKind::DoubleValue(d)) => serde_json::Number::from_f64(d).map(serde_json::Value::Number).unwrap_or(serde_json::Value::Null),
                Some(QdrantValueKind::IntegerValue(i)) => serde_json::Value::Number(i.into()),
                Some(QdrantValueKind::StringValue(s)) => serde_json::Value::String(s),
                Some(QdrantValueKind::ListValue(list)) => {
                    // Recursively convert list elements
                    let json_list: Vec<serde_json::Value> = list.values.into_iter()
                        .filter_map(|v| Self::qdrant_payload_to_serde_value(std::collections::HashMap::from([("inner".to_string(), v)])))
                        .map(|v| v.get("inner").unwrap_or(&serde_json::Value::Null).clone())
                        .collect();
                    serde_json::Value::Array(json_list)
                }
                 Some(QdrantValueKind::StructValue(s)) => {
                     // Recursively convert struct fields
                    let inner_map = s.fields;
                    Self::qdrant_payload_to_serde_value(inner_map)? // Use Option chaining
                 }
                None => serde_json::Value::Null,
            };
            json_map.insert(key, json_value);
        }
        Some(serde_json::Value::Object(json_map))
    }
}

#[async_trait]
impl VectorRepository for VectorDb {
    /// Implements upsert_documents from the trait.
    async fn upsert_documents(&self, documents: &[DocumentToUpsert]) -> Result<()> {
        // Delegate to the implementation method
        self.upsert_documents_impl(documents).await
    }

    /// Implements search from the trait.
    async fn search(&self, query_vector: Vec<f32>, limit: usize, score_threshold: Option<f32>) -> Result<Vec<SearchResult>> {
        // Delegate to the implementation method
        self.search_impl(query_vector, limit, score_threshold).await
    }
}

#[cfg(test)]
mod vector_db_infra_tests {
    use super::*;
    use tokio;
    use testcontainers_modules::qdrant::QdrantImage;
    use testcontainers::clients::Cli;
    use testcontainers::core::{WaitFor, ContainerPort, ContainerAsync, IntoContainerPort};
    use testcontainers::{GenericImage, ImageExt};
    use serial_test::serial; // Add serial_test
    use tempfile::tempdir; // Required for some tests

    // Helper function to set up Qdrant container for tests
    #[serial] // Ensure tests using the container run serially
    async fn setup_qdrant_container() -> (ContainerAsync<GenericImage>, String) { // Return container and URL
        let docker = Cli::default();
        let image = GenericImage::new("qdrant/qdrant", "latest") // Use official image
            .with_wait_for(WaitFor::message_on_stderr("Actix runtime found; starting in Actix runtime"))
            .with_exposed_port(6333.tcp()) // Qdrant standard gRPC port
            .with_exposed_port(6334.tcp()); // Qdrant standard HTTP/REST port

        let container = docker.run(image).await;
        let http_port = container.get_host_port_ipv4(6334).await;
        let qdrant_url = format!("http://localhost:{}", http_port);
        (container, qdrant_url)
    }

    // Basic test for new and initialize
    #[tokio::test]
    #[serial]
    async fn test_vector_db_new_and_initialize() {
        let (_container, qdrant_url) = setup_qdrant_container().await;
        let client = Qdrant::from_url(&qdrant_url).build().expect("Failed to create Qdrant client");
        let vector_db = VectorDb::new(Box::new(client), "test_collection_init".to_string(), 3).expect("Failed to create VectorDb");

        let init_result = vector_db.initialize_collection().await;
        assert!(init_result.is_ok(), "Initialize collection failed: {:?}", init_result.err());

        // Try initializing again, should succeed
        let init_again_result = vector_db.initialize_collection().await;
        assert!(init_again_result.is_ok(), "Initialize collection again failed: {:?}", init_again_result.err());
    }

    // Test upserting and searching
    #[tokio::test]
    #[serial]
    async fn test_vector_db_upsert_and_search() {
        let (_container, qdrant_url) = setup_qdrant_container().await;
        let client = Qdrant::from_url(&qdrant_url).build().expect("Failed to create Qdrant client");
        let collection_name = format!("test_coll_{}", Uuid::new_v4());
        let vector_size: u64 = 3;
        let vector_db = VectorDb::new(Box::new(client), collection_name.clone(), vector_size).expect("Failed to create VectorDb");

        vector_db.initialize_collection().await.expect("Collection initialization failed");

        // Prepare documents to upsert
        let docs_to_upsert = vec![
            DocumentToUpsert {
                file_path: "file1.md".to_string(),
                vector: vec![0.1, 0.2, 0.7], // Example vector
                source: Some("source_A".to_string()),
                content_chunk: "This is chunk 1 from source A.".to_string(),
                metadata: Some(serde_json::json!({ "section": "intro" })),
            },
            DocumentToUpsert {
                file_path: "file2.md".to_string(),
                vector: vec![0.8, 0.1, 0.1], // Example vector
                source: Some("source_B".to_string()),
                content_chunk: "This is chunk 1 from source B.".to_string(),
                metadata: None,
            },
             DocumentToUpsert {
                file_path: "file1.md".to_string(), // Same file, different chunk/source
                vector: vec![0.2, 0.3, 0.5], // Example vector
                source: Some("source_A".to_string()),
                content_chunk: "This is chunk 2 from source A.".to_string(),
                metadata: Some(serde_json::json!({ "section": "details" })),
            },
        ];

        // Use the trait method for testing
        let upsert_result = vector_db.upsert_documents(&docs_to_upsert).await;
        assert!(upsert_result.is_ok(), "Upsert failed: {:?}", upsert_result.err());

        // Allow Qdrant some time to index (important!)
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        // Search for a vector close to the first document
        let query_vector = vec![0.15, 0.25, 0.6];
        let search_result = vector_db.search(query_vector.clone(), 5, Some(0.5)).await;

        assert!(search_result.is_ok(), "Search failed: {:?}", search_result.err());
        let results = search_result.unwrap();

        println!("Search Results: {:?}", results); // Debug output

        assert!(!results.is_empty(), "Search should return results");
        assert_eq!(results.len(), 2, "Expected 2 results above threshold 0.5"); // Adjust expected count based on vectors/threshold

        // Check the content of the top result (closest to query_vector)
        let top_result = &results[0];
        assert!(top_result.score > 0.5, "Score should be above threshold"); // Check score relative to others if needed
        assert_eq!(top_result.file_path, "file1.md");
        assert_eq!(top_result.source, Some("source_A".to_string()));
        assert_eq!(top_result.content_chunk, "This is chunk 1 from source A.");
        assert_eq!(top_result.metadata, Some(serde_json::json!({ "section": "intro" })));

        // Check the second result
        let second_result = &results[1];
         assert!(second_result.score > 0.5);
         assert_eq!(second_result.file_path, "file1.md");
         assert_eq!(second_result.source, Some("source_A".to_string()));
         assert_eq!(second_result.content_chunk, "This is chunk 2 from source A.");
          assert_eq!(second_result.metadata, Some(serde_json::json!({ "section": "details" })));

        // Search for a vector close to the second document
        let query_vector_b = vec![0.7, 0.15, 0.15];
        let search_result_b = vector_db.search(query_vector_b.clone(), 5, Some(0.5)).await.unwrap();
        assert_eq!(search_result_b.len(), 1, "Expected 1 result for query B");
        assert_eq!(search_result_b[0].file_path, "file2.md");
        assert_eq!(search_result_b[0].source, Some("source_B".to_string()));
        assert_eq!(search_result_b[0].content_chunk, "This is chunk 1 from source B.");
        assert!(search_result_b[0].metadata.is_none());

    }

    // Test invalid parameters for new()
    #[tokio::test]
    async fn test_vector_db_new_invalid_params() {
        let (_container, qdrant_url) = setup_qdrant_container().await;
        let client = Qdrant::from_url(&qdrant_url).build().expect("Failed to create Qdrant client");
        assert!(VectorDb::new(Box::new(client.clone()), "".to_string(), 3).is_err()); // Empty collection name
        assert!(VectorDb::new(Box::new(client), "test".to_string(), 0).is_err()); // Zero vector size
    }

    // Test search with wrong vector dimension
    #[tokio::test]
    #[serial]
    async fn test_vector_db_search_wrong_dimension() {
         let (_container, qdrant_url) = setup_qdrant_container().await;
         let client = Qdrant::from_url(&qdrant_url).build().expect("Failed to create Qdrant client");
         let collection_name = format!("test_coll_dim_{}", Uuid::new_v4());
         let vector_db = VectorDb::new(Box::new(client), collection_name, 3).expect("Failed to create VectorDb");
         vector_db.initialize_collection().await.expect("Init failed");

         let query_vector = vec![0.1, 0.2]; // Wrong dimension (2 instead of 3)
         let search_result = vector_db.search(query_vector, 5, None).await;
         assert!(search_result.is_err());
         assert!(search_result.unwrap_err().to_string().contains("Query vector dimension"));
     }

     // Test upserting an empty list
     #[tokio::test]
     #[serial]
     async fn test_vector_db_upsert_empty() {
         let (_container, qdrant_url) = setup_qdrant_container().await;
         let client = Qdrant::from_url(&qdrant_url).build().expect("Failed to create Qdrant client");
         let collection_name = format!("test_coll_empty_{}", Uuid::new_v4());
         let vector_db = VectorDb::new(Box::new(client), collection_name, 3).expect("Failed to create VectorDb");
         vector_db.initialize_collection().await.expect("Init failed");

         let empty_docs: Vec<DocumentToUpsert> = vec![];
         let upsert_result = vector_db.upsert_documents(&empty_docs).await;
         assert!(upsert_result.is_ok(), "Upserting empty list should succeed without error");
     }

    // TODO: Add test case for serialization/deserialization errors during upsert/search
    // TODO: Add test case for search result filtering by source (requires modifying search args/logic)
}

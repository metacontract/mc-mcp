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

mod payload {
    use serde::{Deserialize, Serialize};
    #[derive(Serialize, Deserialize, Debug, Clone)]
    pub struct DocumentPayload {
        pub file_path: String,
        pub source: String,
    }
}
pub use self::payload::DocumentPayload;


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
    ///
    /// # Arguments
    ///
    /// * `documents` - A slice of `DocumentToUpsert` containing file paths and vectors.
    ///
    /// # Returns
    ///
    /// A Result indicating success or failure. Uses anyhow::Error.
    pub async fn upsert_documents(&self, documents: &[DocumentToUpsert]) -> Result<()> {
        if documents.is_empty() {
            log::info!("No documents provided for upsert.");
            return Ok(());
        }

        log::info!("Preparing to upsert {} documents into collection '{}'...", documents.len(), self.collection_name);

        let points: Vec<PointStruct> = documents
            .iter()
            .filter_map(|doc| { // Use filter_map to skip points with errors
                let payload_struct = DocumentPayload {
                    file_path: doc.file_path.clone(),
                    source: doc.source.clone(),
                };
                let payload: Payload = match serde_json::to_value(payload_struct) {
                    Ok(serde_value) => match Payload::try_from(serde_value) {
                        Ok(p) => p,
                        Err(e) => {
                            log::error!("Failed to convert serde_json::Value to Qdrant Payload for file '{}': {}", doc.file_path, e);
                            return None; // Skip this point
                        }
                    },
                    Err(e) => {
                        log::error!("Failed to serialize DocumentPayload for file '{}': {}", doc.file_path, e);
                        return None; // Skip this point
                    }
                };

                let point_id: PointId = PointId::from(Uuid::new_v4().to_string());

                Some(PointStruct {
                    id: Some(point_id),
                    vectors: Some(Vectors::from(doc.vector.clone())),
                    payload: payload.into(), // Payload is already the correct type
                })
            })
            .collect();

        if points.is_empty() {
             log::warn!("No valid points could be prepared for upserting (input count: {}). Check serialization/conversion errors.", documents.len());
             return Ok(());
        }

        // Log before moving points
        let points_count = points.len();
        log::info!("Upserting {} valid points into collection '{}'...", points_count, self.collection_name);

        // Use the builder pattern for UpsertPoints
        let upsert_builder = UpsertPointsBuilder::new(self.collection_name.clone(), points) // Move points here
             .wait(true);

        match self.client.upsert_points(upsert_builder).await { // Pass the builder
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

    /// Searches for documents in the Qdrant collection based on a query vector.
    ///
    /// # Arguments
    ///
    /// * `query_vector` - The vector to search with.
    /// * `limit` - The maximum number of results to return.
    /// * `score_threshold` - Optionally filter out results below this similarity score.
    ///
    /// # Returns
    ///
    /// A Result containing a vector of `ScoredPoint` on success, or an error on failure. Uses anyhow::Error.
    pub async fn search(&self, query_vector: Vec<f32>, limit: usize, score_threshold: Option<f32>) -> Result<Vec<ScoredPoint>> {
         if query_vector.len() as u64 != self.vector_size {
            return Err(anyhow!(
                "Query vector dimension ({}) does not match collection dimension ({})",
                query_vector.len(),
                self.vector_size
            ));
        }

        log::info!("Searching in collection '{}' with limit {}...", self.collection_name, limit);

        // SearchPoints struct can be passed directly (it implements Into<SearchPoints>)
        let search_request = SearchPoints {
            collection_name: self.collection_name.clone(),
            vector: query_vector,
            limit: limit as u64,
            with_payload: Some(WithPayloadSelector { // Request payload
                 selector_options: Some(qdrant_client::qdrant::with_payload_selector::SelectorOptions::Enable(true)),
            }),
            with_vectors: Some(WithVectorsSelector{ // Don't request vectors
                 selector_options: Some(qdrant_client::qdrant::with_vectors_selector::SelectorOptions::Enable(false)),
            }),
            // filter: None, // Add filter if needed
            score_threshold, // 追加
            ..Default::default()
        };

        log::debug!("Sending search request: {:?}", search_request);

        // Pass the search_request struct directly
        match self.client.search_points(search_request).await {
            Ok(response) => {
                log::info!("Search completed successfully, found {} results.", response.result.len());
                Ok(response.result)
            }
            Err(e) => {
                 log::error!("Qdrant search failed in collection '{}': {}", self.collection_name, e);
                Err(anyhow!("Qdrant search failed: {}", e))
            }
        }
    }
}

#[async_trait]
impl VectorRepository for VectorDb {
    /// Implements the trait method by calling the struct's concrete method.
    async fn upsert_documents(&self, documents: &[DocumentToUpsert]) -> Result<()> {
        // Call the struct's own upsert_documents method
        self.upsert_documents(documents).await
    }

    /// Implements the trait method by calling the struct's concrete method.
    async fn search(&self, query_vector: Vec<f32>, limit: usize, score_threshold: Option<f32>) -> Result<Vec<ScoredPoint>> {
        // Call the struct's own search method which already returns Vec<ScoredPoint>
        self.search(query_vector, limit, score_threshold).await
    }
}

#[cfg(test)]
mod vector_db_tests {
    use super::*;
    use testcontainers::{
        core::{IntoContainerPort, WaitFor},
        runners::AsyncRunner,
        GenericImage, ContainerAsync
    };
    use tokio;
    use serial_test::serial; // Import the serial attribute macro
    use std::collections::HashMap; // Import HashMap for unwrap_or_else
    // Use crate path for qdrant types since qdrant_client is re-exported
    use super::qdrant_client::qdrant::CollectionStatus;
    use simple_logger; // Add import for logger

    const TEST_COLLECTION_NAME: &str = "test_integration_collection";
    const QDRANT_IMAGE_NAME: &str = "qdrant/qdrant";
    const QDRANT_IMAGE_TAG: &str = "latest";
    const QDRANT_GRPC_PORT: u16 = 6334;
    const TEST_VECTOR_DIM: u64 = 4;

    // Helper function to setup and run the Qdrant container and return the gRPC URL
    async fn setup_qdrant_container() -> (ContainerAsync<GenericImage>, String) { // Return container and URL
        // Initialize logger for tests
        // Use try_init to avoid panic if logger is already initialized
        let _ = simple_logger::SimpleLogger::new().with_level(log::LevelFilter::Info).init();

        log::info!("Starting Qdrant container for integration test...");
        let image = GenericImage::new(QDRANT_IMAGE_NAME, QDRANT_IMAGE_TAG)
            .with_exposed_port(QDRANT_GRPC_PORT.tcp())
            .with_wait_for(WaitFor::message_on_stderr("Qdrant initialization completed"));

        let container = image.start().await.expect("Failed to start Qdrant container");
        let host_port = container.get_host_port_ipv4(QDRANT_GRPC_PORT).await.expect("Failed to get mapped Qdrant port");
        let qdrant_url = format!("http://localhost:{}", host_port);
        log::info!("Qdrant container started, gRPC accessible at: {}", qdrant_url);
        (container, qdrant_url)
    }

    #[tokio::test]
    #[serial]
    async fn test_vector_db_new_and_initialize() {
        let (_container, qdrant_url) = setup_qdrant_container().await;
        let vector_db = VectorDb::new(Box::new(super::qdrant_client::Qdrant::from_url(&qdrant_url).build().expect("Failed to create Qdrant client")), TEST_COLLECTION_NAME.to_string(), TEST_VECTOR_DIM)
            .expect("Failed to create VectorDb instance");

        // Test initialization (should create)
        let init_result = vector_db.initialize_collection().await;
        assert!(init_result.is_ok(), "Failed to initialize collection (create): {:?}", init_result.err());

        // Verify collection exists
        let info_result = vector_db.client.collection_info(TEST_COLLECTION_NAME).await;
        assert!(info_result.is_ok(), "Failed to get collection info after create: {:?}", info_result.err());
        assert_eq!(info_result.unwrap().result.unwrap().status(), CollectionStatus::Green, "Collection status should be Green after create");

        // Test initialization again (should detect existing)
        let init_result_again = vector_db.initialize_collection().await;
        assert!(init_result_again.is_ok(), "Initializing existing collection failed: {:?}", init_result_again.err());

    }


    #[tokio::test]
    #[serial]
    async fn test_vector_db_upsert_and_search() {
        let (_container, qdrant_url) = setup_qdrant_container().await;
        let vector_db = VectorDb::new(Box::new(super::qdrant_client::Qdrant::from_url(&qdrant_url).build().expect("Failed to create Qdrant client")), TEST_COLLECTION_NAME.to_string(), TEST_VECTOR_DIM)
            .expect("Failed to create VectorDb instance");
        vector_db.initialize_collection().await.expect("Collection initialization failed");

        let documents_to_upsert = vec![
            DocumentToUpsert {
                file_path: "file1.md".to_string(),
                vector: vec![0.1, 0.2, 0.3, 0.4],
                source: "test-source".to_string(),
            },
            DocumentToUpsert {
                file_path: "file2.txt".to_string(),
                vector: vec![0.5, 0.6, 0.7, 0.8],
                source: "test-source".to_string(),
            },
            DocumentToUpsert {
                file_path: "subdir/file3.md".to_string(),
                vector: vec![0.9, 0.8, 0.7, 0.6],
                source: "test-source".to_string(),
            },
             // Add a document that will fail payload serialization
             DocumentToUpsert {
                file_path: "invalid_payload_doc.txt".to_string(),
                vector: vec![0.0, 0.0, 0.0, 0.0],
                source: "test-source".to_string(),
                // Payload struct that might cause issues if not handled correctly
                // Let's assume Payload::try_from would fail for some complex nested value
                // For simplicity, we rely on the existing filter_map in upsert_documents
            },
        ];

        // Test upsert
        let upsert_result = vector_db.upsert_documents(&documents_to_upsert).await;
        assert!(upsert_result.is_ok(), "Upsert failed: {:?}", upsert_result.err());

        // Give Qdrant time to index
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Verify point count (should be 3, skipping the invalid one)
        let count_builder = super::qdrant_client::qdrant::CountPointsBuilder::new(TEST_COLLECTION_NAME)
            .exact(true);
        let count_response = vector_db.client.count(count_builder).await.expect("Count request failed");
        assert_eq!(count_response.result.expect("Count result missing").count, 3, "Should have 3 points after upsert (one skipped)");

        // Test search (find vector similar to file1)
        let query_vector = vec![0.11, 0.21, 0.31, 0.41];
        let search_result = vector_db.search(query_vector.clone(), 3, None).await;
        assert!(search_result.is_ok(), "Search failed: {:?}", search_result.err());
        let results = search_result.unwrap();
        assert!(!results.is_empty(), "Search returned no results");
        assert_eq!(results.len(), 3, "Search should return 3 results (limit)");

        let top_result = &results[0];
        let top_payload_value = Payload::from(top_result.payload.clone()).into(); // Convert to serde_json::Value
        let top_payload: DocumentPayload = serde_json::from_value(top_payload_value)
                                           .expect("Failed to deserialize payload");
        assert_eq!(top_payload.file_path, "file1.md", "Top search result mismatch");
        log::info!("Search results for {:?}: {:?}", query_vector, results);

        // Test search (find vector similar to file2)
        let query_vector_2 = vec![0.55, 0.65, 0.75, 0.85];
        let search_result_2 = vector_db.search(query_vector_2.clone(), 1, None).await;
        assert!(search_result_2.is_ok(), "Search 2 failed: {:?}", search_result_2.err());
        let results_2 = search_result_2.unwrap();
        assert_eq!(results_2.len(), 1, "Search 2 should return 1 result");
        let top_result_2 = &results_2[0];
        let top_payload_value_2 = Payload::from(top_result_2.payload.clone()).into(); // Convert to serde_json::Value
        let top_payload_2: DocumentPayload = serde_json::from_value(top_payload_value_2)
                                           .expect("Failed to deserialize payload 2");
        assert_eq!(top_payload_2.file_path, "file2.txt", "Top search result 2 mismatch");
    }

    #[tokio::test]
    #[serial]
     async fn test_vector_db_new_invalid_params() {
         let (_container, qdrant_url) = setup_qdrant_container().await;
         let vector_db = VectorDb::new(Box::new(super::qdrant_client::Qdrant::from_url(&qdrant_url).build().expect("Failed to create Qdrant client")), "".to_string(), TEST_VECTOR_DIM);
         assert!(vector_db.is_err());

         let vector_db = VectorDb::new(Box::new(super::qdrant_client::Qdrant::from_url(&qdrant_url).build().expect("Failed to create Qdrant client")), TEST_COLLECTION_NAME.to_string(), 0);
         assert!(vector_db.is_err());
     }

    #[tokio::test]
    #[serial]
    async fn test_vector_db_search_wrong_dimension() {
         let (_container, qdrant_url) = setup_qdrant_container().await;
         let vector_db = VectorDb::new(Box::new(super::qdrant_client::Qdrant::from_url(&qdrant_url).build().expect("Failed to create Qdrant client")), TEST_COLLECTION_NAME.to_string(), TEST_VECTOR_DIM)
             .expect("Failed to create VectorDb instance");
         vector_db.initialize_collection().await.expect("Collection initialization failed");

         // Test search with wrong dimension
         let wrong_dim_vector = vec![0.1, 0.2, 0.3]; // Dim 3 instead of 4
         let search_result = vector_db.search(wrong_dim_vector, 1, None).await;
         assert!(search_result.is_err(), "Search with wrong dimension should fail");
     }

    #[tokio::test]
    #[serial]
     async fn test_vector_db_upsert_empty() {
         let (_container, qdrant_url) = setup_qdrant_container().await;
         let vector_db = VectorDb::new(Box::new(super::qdrant_client::Qdrant::from_url(&qdrant_url).build().expect("Failed to create Qdrant client")), TEST_COLLECTION_NAME.to_string(), TEST_VECTOR_DIM)
             .expect("Failed to create VectorDb instance");
         vector_db.initialize_collection().await.expect("Collection initialization failed");

         let empty_docs: Vec<DocumentToUpsert> = vec![];
         let upsert_result = vector_db.upsert_documents(&empty_docs).await;
         assert!(upsert_result.is_ok(), "Upserting empty slice failed");

         // Verify point count is 0
         let count_builder = super::qdrant_client::qdrant::CountPointsBuilder::new(TEST_COLLECTION_NAME)
             .exact(true);
         let count_response = vector_db.client.count(count_builder).await.expect("Count request failed");
         assert_eq!(count_response.result.expect("Count result missing").count, 0, "Should have 0 points after upserting empty slice");

         let search_result = vector_db.search(vec![0.1, 0.2, 0.3, 0.4], 1, None).await;
         assert!(search_result.is_ok(), "Search after empty upsert failed");
         assert!(search_result.unwrap().is_empty(), "Search should return empty results after empty upsert");
     }
}

// NOTE: use statements will need adjustment after moving files
// Remove unused Serialize/Deserialize here (used in payload module)
// use serde::{Deserialize, Serialize};
use log;
// Use the re-exported module path for Qdrant internally
use self::qdrant_client::Qdrant;
use self::qdrant_client::{
    qdrant::{
        CreateCollectionBuilder, Distance, PointId, PointStruct, SearchPoints, UpsertPointsBuilder,
        VectorParams, Vectors, WithPayloadSelector, WithVectorsSelector,
    },
    Payload,
};
pub use qdrant_client; // Re-export the entire module
                       // Remove unused ScoredPoint import
                       // use self::qdrant_client::qdrant::ScoredPoint;
use anyhow::{anyhow, Result};
use uuid::Uuid;
// Import serde::Deserialize for DocumentToUpsert
use serde::Deserialize;
use serde::Serialize;
// Removed unused serial_test import here

// Required imports for shared container logic
// Remove Lazy import
// use tokio::sync::Lazy; // Use tokio's Lazy for async static init
use async_trait::async_trait;
// use testcontainers::core::WaitFor; // WaitFor is in core
// use testcontainers::runners::AsyncRunner;
// use testcontainers::{ContainerAsync, GenericImage}; // Correct import for GenericImage and ContainerAsync
// use tokio::sync::OnceCell; // Use OnceCell for async initialization // Add missing trait import

// Remove unused DocumentToUpsert from embedding import
// use super::embedding::DocumentToUpsert;
// Assuming VectorRepository trait is in domain/vector_repository.rs
use self::qdrant_client::qdrant::value::Kind as QdrantValueKind;
use crate::domain::reference::SearchResult;
use crate::domain::vector_repository::VectorRepository; // Import domain SearchResult

mod payload {
    use serde::{Deserialize, Serialize};
    #[derive(Serialize, Deserialize, Debug, Clone)]
    pub struct DocumentPayload {
        pub file_path: String,
        pub source: Option<String>, // Make optional to match SearchResult
        pub content_chunk: String,  // Add the chunk content
        pub metadata: Option<serde_json::Value>, // Add optional metadata
    }
}
pub use self::payload::DocumentPayload;

// --- Document to Upsert --- (Keep this definition)
// Structure passed to upsert_documents. Contains data needed to create PointStruct.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DocumentToUpsert {
    pub file_path: String,
    pub vector: Vec<f32>,
    pub source: Option<String>,              // Make optional
    pub content_chunk: String,               // Add content chunk
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
        Ok(Self {
            client,
            collection_name,
            vector_size,
        })
    }

    /// Initializes the Qdrant collection if it doesn't exist.
    ///
    /// # Returns
    ///
    /// A Result indicating success or failure.
    pub async fn initialize_collection(&self) -> Result<()> {
        log::info!(
            "Checking if collection '{}' exists...",
            self.collection_name
        );

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
        log::info!(
            "Creating collection '{}' with size {} and distance Cosine...",
            self.collection_name,
            self.vector_size
        );

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
                log::info!(
                    "Successfully created collection '{}'.",
                    self.collection_name
                );
                Ok(())
            }
            Err(e) => {
                log::error!(
                    "Failed to create collection '{}': {}",
                    self.collection_name,
                    e
                );
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

        log::info!(
            "Preparing to upsert {} documents into collection '{}'...",
            documents.len(),
            self.collection_name
        );

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
        log::info!(
            "Upserting {} valid points into collection '{}'...",
            points_count,
            self.collection_name
        );

        let upsert_builder =
            UpsertPointsBuilder::new(self.collection_name.clone(), points).wait(true);

        match self.client.upsert_points(upsert_builder).await {
            Ok(response) => {
                log::debug!("Upsert response: {:?}", response);
                if let Some(result) = response.result {
                    log::info!(
                        "Upsert operation completed with status: {:?}",
                        result.status()
                    );
                } else {
                    log::warn!("Upsert response did not contain result details.");
                }
                // Use the stored count
                log::info!("Successfully requested upsert for {} points.", points_count);
                Ok(())
            }
            Err(e) => {
                log::error!(
                    "Failed to upsert points into collection '{}': {}",
                    self.collection_name,
                    e
                );
                Err(anyhow!("Qdrant upsert failed: {}", e))
            }
        }
    }

    /// Searches for documents in the Qdrant collection and returns domain SearchResult.
    pub async fn search_impl(
        &self,
        query_vector: Vec<f32>,
        limit: usize,
        score_threshold: Option<f32>,
    ) -> Result<Vec<SearchResult>> {
        if query_vector.len() as u64 != self.vector_size {
            return Err(anyhow!(
                "Query vector dimension ({}) does not match collection dimension ({})",
                query_vector.len(),
                self.vector_size
            ));
        }

        log::info!(
            "Searching in collection '{}' with limit {}...",
            self.collection_name,
            limit
        );

        let search_request = SearchPoints {
            collection_name: self.collection_name.clone(),
            vector: query_vector,
            limit: limit as u64,
            with_payload: Some(WithPayloadSelector {
                // Request payload
                selector_options: Some(
                    qdrant_client::qdrant::with_payload_selector::SelectorOptions::Enable(true),
                ),
            }),
            with_vectors: Some(WithVectorsSelector {
                // Don't need vectors in result
                selector_options: Some(
                    qdrant_client::qdrant::with_vectors_selector::SelectorOptions::Enable(false),
                ),
            }),
            score_threshold,
            ..Default::default()
        };

        log::debug!("Sending search request: {:?}", search_request);

        match self.client.search_points(search_request).await {
            Ok(response) => {
                log::info!(
                    "Search completed successfully, found {} potential results.",
                    response.result.len()
                );

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

                log::info!(
                    "Successfully mapped {} results to SearchResult.",
                    search_results.len()
                );
                Ok(search_results)
            }
            Err(e) => {
                log::error!(
                    "Qdrant search failed in collection '{}': {}",
                    self.collection_name,
                    e
                );
                Err(anyhow!("Qdrant search failed: {}", e))
            }
        }
    }

    // Helper function to convert Qdrant Payload map to serde_json::Value
    // Returns Option<Value> because conversion might fail for a point
    fn qdrant_payload_to_serde_value(
        payload_map: std::collections::HashMap<String, qdrant_client::qdrant::Value>,
    ) -> Option<serde_json::Value> {
        let mut json_map = serde_json::Map::new();
        for (key, qdrant_value) in payload_map {
            let json_value = match qdrant_value.kind {
                Some(QdrantValueKind::NullValue(_)) => serde_json::Value::Null,
                Some(QdrantValueKind::BoolValue(b)) => serde_json::Value::Bool(b),
                Some(QdrantValueKind::DoubleValue(d)) => serde_json::Number::from_f64(d)
                    .map(serde_json::Value::Number)
                    .unwrap_or(serde_json::Value::Null),
                Some(QdrantValueKind::IntegerValue(i)) => serde_json::Value::Number(i.into()),
                Some(QdrantValueKind::StringValue(s)) => serde_json::Value::String(s),
                Some(QdrantValueKind::ListValue(list)) => {
                    // Recursively convert list elements
                    let json_list: Vec<serde_json::Value> = list
                        .values
                        .into_iter()
                        .filter_map(|v| {
                            Self::qdrant_payload_to_serde_value(std::collections::HashMap::from([
                                ("inner".to_string(), v),
                            ]))
                        })
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
    async fn search(
        &self,
        query_vector: Vec<f32>,
        limit: usize,
        score_threshold: Option<f32>,
    ) -> Result<Vec<SearchResult>> {
        // Delegate to the implementation method
        self.search_impl(query_vector, limit, score_threshold).await
    }
}

pub fn add(left: u64, right: u64) -> u64 {
    left + right
}

// Define modules first
mod document_index {
    use std::collections::HashMap;
    pub type SimpleDocumentIndex = HashMap<String, String>;
}
mod payload {
    use serde::{Deserialize, Serialize};
    #[derive(Serialize, Deserialize, Debug, Clone)]
    pub struct DocumentPayload {
        pub file_path: String,
    }
}

// Re-export necessary types and the qdrant_client module
pub use fastembed::EmbeddingModel;
pub use qdrant_client; // Re-export the entire module
pub use self::document_index::SimpleDocumentIndex;
pub use self::payload::DocumentPayload;

// Internal use statements
use comrak::{markdown_to_html, ComrakOptions};
use std::fs;
use std::path::PathBuf;
use walkdir::WalkDir;
use fastembed::{TextEmbedding, InitOptions, Error as FastEmbedError};
use log;
// Use the re-exported module path for Qdrant internally
use self::qdrant_client::Qdrant;
use self::qdrant_client::{
    Payload,
    qdrant::{PointStruct, ScoredPoint, SearchPoints, PointId, Vectors, WithPayloadSelector, WithVectorsSelector, Distance, VectorParams, CreateCollectionBuilder, UpsertPointsBuilder},
};
use uuid::Uuid;
use anyhow::{anyhow, Result};

/// Parses a Markdown string and returns its plain text representation.
///
/// This function converts the Markdown to HTML first, then extracts the text content.
/// It uses default Comrak options.
///
/// # Arguments
///
/// * `markdown` - A string slice containing the Markdown text.
///
/// # Returns
///
/// A String containing the plain text extracted from the Markdown.
pub fn parse_markdown_to_text(markdown: &str) -> String {
    // TODO: より効率的なテキスト抽出方法を検討 (HTMLを経由しない方法)
    let html = markdown_to_html(markdown, &ComrakOptions::default());

    // HTML からテキストを抽出 (簡易的な方法)
    // より堅牢なライブラリ (例: scraper) の使用も検討できる
    html_to_text(&html)
}

// HTML文字列からタグを除去してテキストを抽出するヘルパー関数 (簡易版)
fn html_to_text(html: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(c),
            _ => {},
        }
    }
    // 簡単な空白の整形
    result.split_whitespace().collect::<Vec<&str>>().join(" ")
}

/// Loads Markdown documents from a specified directory (or default) and creates a simple index.
///
/// Recursively searches for `.md` files in the given directory, reads them,
/// parses them to plain text, and stores them in a HashMap.
///
/// # Arguments
///
/// * `docs_path` - An optional PathBuf specifying the directory to load documents from.
///                 If None, defaults to "metacontract/mc/site/docs".
///
/// # Returns
///
/// A Result containing the `SimpleDocumentIndex` on success, or a String error message on failure.
pub fn load_documents(docs_path: Option<PathBuf>) -> Result<SimpleDocumentIndex, String> {
    let default_path = PathBuf::from("metacontract/mc/site/docs");
    let target_path = docs_path.unwrap_or(default_path);

    println!("Loading documents from: {:?}", target_path);

    if !target_path.is_dir() {
        return Err(format!("Specified path is not a directory: {:?}", target_path));
    }

    let mut index = SimpleDocumentIndex::new();

    for entry in WalkDir::new(&target_path)
        .into_iter()
        .filter_map(|e| e.ok()) // エラーになったエントリは無視
        .filter(|e| e.path().is_file() && e.path().extension().map_or(false, |ext| ext == "md"))
    {
        let path = entry.path();
        let path_str = path.to_string_lossy().to_string();

        match fs::read_to_string(path) {
            Ok(content) => {
                let text = parse_markdown_to_text(&content);
                index.insert(path_str, text);
            }
            Err(e) => {
                eprintln!("Failed to read file {}: {}", path_str, e);
            }
        }
    }

    if index.is_empty() {
       println!("Warning: No markdown files found or loaded from {:?}", target_path);
    }

    Ok(index)
}

/// A struct responsible for generating text embeddings using a pre-initialized model.
pub struct EmbeddingGenerator {
    model: TextEmbedding,
}

impl EmbeddingGenerator {
    /// Creates a new EmbeddingGenerator, initializing the specified embedding model.
    ///
    /// This function might block while downloading the model files for the first time.
    ///
    /// # Arguments
    ///
    /// * `model_name` - The embedding model to use (e.g., EmbeddingModel::AllMiniLML6V2).
    ///
    /// # Returns
    ///
    /// A Result containing the `EmbeddingGenerator` on success, or a `FastEmbedError` on failure.
    pub fn new(model_name: EmbeddingModel) -> Result<Self, FastEmbedError> {
        let model = TextEmbedding::try_new(InitOptions::new(model_name))?;
        Ok(EmbeddingGenerator { model })
    }

    /// Generates embeddings for a batch of documents.
    ///
    /// # Arguments
    ///
    /// * `documents` - A slice of string slices representing the documents to embed.
    ///
    /// # Returns
    ///
    /// A Result containing a vector of embedding vectors (Vec<Vec<f32>>) on success,
    /// or a `FastEmbedError` on failure.
    pub fn generate_embeddings(&self, documents: &[&str]) -> Result<Vec<Vec<f32>>, FastEmbedError> {
        self.model.embed(documents.to_vec(), None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn it_works() {
        let result = add(2, 2);
        assert_eq!(result, 4);
    }

    #[test]
    fn test_parse_simple_markdown() {
        let markdown = "# Header\n\nThis is **bold** text.";
        let expected_text = "Header This is bold text.";
        assert_eq!(parse_markdown_to_text(markdown), expected_text);
    }

    #[test]
    fn test_parse_markdown_with_link() {
        let markdown = "Visit [Google](https://google.com)!";
        let expected_text = "Visit Google!";
        assert_eq!(parse_markdown_to_text(markdown), expected_text);
    }

    #[test]
    fn test_parse_markdown_with_list() {
        let markdown = "* Item 1\n* Item 2";
        let expected_text = "Item 1 Item 2";
        assert_eq!(parse_markdown_to_text(markdown), expected_text);
    }

    #[test]
    fn test_load_documents_default_path_not_exists() {
        let result = load_documents(None);
        assert!(result.is_err());
    }

    #[test]
    fn test_load_documents_from_temp_dir() {
        let dir = tempdir().unwrap();
        let docs_path = dir.path().to_path_buf();

        fs::create_dir(docs_path.join("sub")).unwrap();
        let mut file1 = File::create(docs_path.join("file1.md")).unwrap();
        writeln!(file1, "# Title 1\nContent 1").unwrap();
        let mut file2 = File::create(docs_path.join("sub/file2.md")).unwrap();
        writeln!(file2, "* List item").unwrap();
        let mut file3 = File::create(docs_path.join("not_markdown.txt")).unwrap();
        writeln!(file3, "ignore me").unwrap();

        let index = load_documents(Some(docs_path.clone())).unwrap();

        assert_eq!(index.len(), 2);
        assert_eq!(index.get(&docs_path.join("file1.md").to_string_lossy().to_string()), Some(&"Title 1 Content 1".to_string()));
        assert_eq!(index.get(&docs_path.join("sub/file2.md").to_string_lossy().to_string()), Some(&"List item".to_string()));
        assert!(!index.contains_key(&docs_path.join("not_markdown.txt").to_string_lossy().to_string()));

        drop(file1);
        drop(file2);
        drop(file3);
        dir.close().unwrap();
    }

     #[test]
    fn test_load_documents_empty_dir() {
        let dir = tempdir().unwrap();
        let docs_path = dir.path().to_path_buf();

        let index = load_documents(Some(docs_path)).unwrap();
        assert!(index.is_empty());

        dir.close().unwrap();
    }

    #[test]
    fn test_load_documents_non_existent_dir() {
        let path = PathBuf::from("non_existent_dir_for_test");
        let result = load_documents(Some(path));
        assert!(result.is_err());
    }

    #[test]
    fn test_embedding_generator_init_and_embed() {
        // EmbeddingModel::Default を BGESmallENV15 に変更
        let generator_result = EmbeddingGenerator::new(EmbeddingModel::BGESmallENV15);
        if let Err(e) = &generator_result {
            println!("Warning: Failed to initialize EmbeddingGenerator (might be due to download issue): {}", e);
            return;
        }
        let generator = generator_result.unwrap();

        let documents = vec!["hello world", "this is a test"];
        let embeddings_result = generator.generate_embeddings(&documents);

        match embeddings_result {
            Ok(embeddings) => {
                assert_eq!(embeddings.len(), 2);
                assert!(!embeddings[0].is_empty());
                assert!(!embeddings[1].is_empty());
                println!("Generated embedding dimension: {}", embeddings[0].len());
                // BGESmallENV15 の次元数は 384
                assert_eq!(embeddings[0].len(), 384);
                assert_eq!(embeddings[1].len(), 384);
            }
            Err(e) => {
                panic!("Embedding generation failed: {}", e);
            }
        }
    }
}

// --- Add Document structures below ---

/// Represents a document ready to be upserted into Qdrant.
#[derive(Debug, Clone)]
pub struct DocumentToUpsert {
    pub file_path: String,
    pub vector: Vec<f32>,
    // Text content might be useful here too for context, but payload only needs file_path for now
    // pub text_content: String,
}

/// Represents the Vector Database client and configuration.
pub struct VectorDb {
    client: Qdrant,
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
    pub fn new(client: Qdrant, collection_name: String, vector_size: u64) -> Result<Self> {
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
        match self.client.collection_info(self.collection_name.clone()).await {
            Ok(info) => {
                 match info.result {
                    Some(collection_info) => {
                        log::info!("Collection '{}' already exists. Status: {:?}", self.collection_name, collection_info.status());
                        Ok(())
                    }
                    None => {
                         log::warn!("Collection info request succeeded but returned no result for '{}'. Assuming it doesn't exist.", self.collection_name);
                         self.create_collection_internal().await
                    }
                 }
            }
            Err(e) => {
                // Attempt to check gRPC status code if the feature is enabled
                #[cfg(feature = "tonic-error-checking")]
                {
                    if let Some(status) = e.tonic_status() { // Assuming QdrantError exposes tonic_status() or similar
                        if status.code() == tonic::Code::NotFound {
                            log::info!("Collection '{}' not found (gRPC status {}). Creating...", self.collection_name, status.code());
                            return self.create_collection_internal().await;
                        }
                    }
                }

                // Fallback check on string representation (less reliable)
                let error_string = e.to_string().to_lowercase(); // Convert to lowercase for case-insensitive comparison
                if error_string.contains("not found") || error_string.contains("doesn't exist") {
                    log::info!("Collection '{}' not found (error message check: '{}'). Creating...", self.collection_name, error_string);
                    return self.create_collection_internal().await;
                }

                log::error!("Failed to get collection info for '{}' due to an unexpected error: {}. Cannot proceed with initialization.", self.collection_name, e);
                Err(anyhow!("Failed to check collection existence: {}", e))

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
    ///
    /// # Returns
    ///
    /// A Result containing a vector of `ScoredPoint` on success, or an error on failure. Uses anyhow::Error.
    pub async fn search(&self, query_vector: Vec<f32>, limit: usize) -> Result<Vec<ScoredPoint>> {
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
            // score_threshold: None, // Add score threshold if needed
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


#[cfg(test)]
#[serial] // Ensure tests run serially due to Docker resource usage
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
    use crate::qdrant_client::qdrant::{CollectionStatus, PointIdChoice}; // Use crate path

    const TEST_COLLECTION_NAME: &str = "test_integration_collection";
    const QDRANT_IMAGE_NAME: &str = "qdrant/qdrant";
    const QDRANT_IMAGE_TAG: &str = "latest";
    const QDRANT_GRPC_PORT: u16 = 6334;
    const TEST_VECTOR_DIM: u64 = 4;

    // Helper function to setup and run the Qdrant container and return the gRPC URL
    async fn setup_qdrant_container() -> (ContainerAsync<GenericImage>, String) { // Return container and URL
        // Initialize logger for tests
        simple_logger::SimpleLogger::new().with_level(log::LevelFilter::Info).init().unwrap_or(());

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
        let client = crate::qdrant_client::Qdrant::from_url(&qdrant_url).build().expect("Failed to create Qdrant client");
        let vector_db = VectorDb::new(client.clone(), TEST_COLLECTION_NAME.to_string(), TEST_VECTOR_DIM)
            .expect("Failed to create VectorDb instance");

        // Test initialization (should create)
        let init_result = vector_db.initialize_collection().await;
        assert!(init_result.is_ok(), "Failed to initialize collection (create): {:?}", init_result.err());

        // Verify collection exists
        let info_result = client.collection_info(TEST_COLLECTION_NAME).await;
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
        let client = crate::qdrant_client::Qdrant::from_url(&qdrant_url).build().expect("Failed to create Qdrant client");
        let vector_db = VectorDb::new(client.clone(), TEST_COLLECTION_NAME.to_string(), TEST_VECTOR_DIM)
            .expect("Failed to create VectorDb instance");
        vector_db.initialize_collection().await.expect("Collection initialization failed");

        let documents_to_upsert = vec![
            DocumentToUpsert {
                file_path: "file1.md".to_string(),
                vector: vec![0.1, 0.2, 0.3, 0.4],
            },
            DocumentToUpsert {
                file_path: "file2.txt".to_string(),
                vector: vec![0.5, 0.6, 0.7, 0.8],
            },
            DocumentToUpsert {
                file_path: "subdir/file3.md".to_string(),
                vector: vec![0.9, 0.8, 0.7, 0.6],
            },
             // Add a document that will fail payload serialization
             DocumentToUpsert {
                file_path: "invalid_payload_doc.txt".to_string(),
                vector: vec![0.0, 0.0, 0.0, 0.0],
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
        let count_response = client.count_points(TEST_COLLECTION_NAME, None, true).await.expect("Count request failed");
        assert_eq!(count_response.result.expect("Count result missing").count, 3, "Should have 3 points after upsert (one skipped)");

        // Test search (find vector similar to file1)
        let query_vector = vec![0.11, 0.21, 0.31, 0.41];
        let search_result = vector_db.search(query_vector.clone(), 3).await;
        assert!(search_result.is_ok(), "Search failed: {:?}", search_result.err());
        let results = search_result.unwrap();
        assert!(!results.is_empty(), "Search returned no results");
        assert_eq!(results.len(), 3, "Search should return 3 results (limit)");

        let top_result = &results[0];
        let top_payload_map = top_result.payload.clone().unwrap_or_else(HashMap::new);
        let top_payload: DocumentPayload = serde_json::from_value(serde_json::Value::Object(top_payload_map.into()))
                                           .expect("Failed to deserialize payload");
        assert_eq!(top_payload.file_path, "file1.md", "Top search result mismatch");
        log::info!("Search results for {:?}: {:?}", query_vector, results);

        // Test search (find vector similar to file2)
        let query_vector_2 = vec![0.55, 0.65, 0.75, 0.85];
        let search_result_2 = vector_db.search(query_vector_2.clone(), 1).await;
        assert!(search_result_2.is_ok(), "Search 2 failed: {:?}", search_result_2.err());
        let results_2 = search_result_2.unwrap();
        assert_eq!(results_2.len(), 1, "Search 2 should return 1 result");
        let top_result_2 = &results_2[0];
        let top_payload_map_2 = top_result_2.payload.clone().unwrap_or_else(HashMap::new);
        let top_payload_2: DocumentPayload = serde_json::from_value(serde_json::Value::Object(top_payload_map_2.into()))
                                           .expect("Failed to deserialize payload 2");
        assert_eq!(top_payload_2.file_path, "file2.txt", "Top search result 2 mismatch");

    }

    // ... (test_vector_db_new_invalid_params, test_vector_db_search_wrong_dimension, test_vector_db_upsert_empty remain largely the same)
     #[tokio::test]
     #[serial]
     async fn test_vector_db_new_invalid_params() {
         let (_container, qdrant_url) = setup_qdrant_container().await;
         let client = crate::qdrant_client::Qdrant::from_url(&qdrant_url).build().expect("Failed to create Qdrant client");

         let result_empty_name = VectorDb::new(client.clone(), "".to_string(), TEST_VECTOR_DIM);
         assert!(result_empty_name.is_err());
         assert!(result_empty_name.unwrap_err().to_string().contains("Collection name cannot be empty"));

         let result_zero_size = VectorDb::new(client.clone(), "valid_name".to_string(), 0);
         assert!(result_zero_size.is_err());
         assert!(result_zero_size.unwrap_err().to_string().contains("Vector size must be greater than zero"));
     }

    #[tokio::test]
    #[serial]
    async fn test_vector_db_search_wrong_dimension() {
        let (_container, qdrant_url) = setup_qdrant_container().await;
        let client = crate::qdrant_client::Qdrant::from_url(&qdrant_url).build().expect("Failed to create Qdrant client");
        let vector_db = VectorDb::new(client, TEST_COLLECTION_NAME.to_string(), TEST_VECTOR_DIM)
            .expect("Failed to create VectorDb instance");
        vector_db.initialize_collection().await.expect("Collection initialization failed");

        let wrong_dim_vector = vec![1.0, 2.0, 3.0];
        let search_result = vector_db.search(wrong_dim_vector, 1).await;
        assert!(search_result.is_err());
        assert!(search_result.unwrap_err().to_string().contains("Query vector dimension"));
    }

     #[tokio::test]
     #[serial]
     async fn test_vector_db_upsert_empty() {
         let (_container, qdrant_url) = setup_qdrant_container().await;
         let client = crate::qdrant_client::Qdrant::from_url(&qdrant_url).build().expect("Failed to create Qdrant client");
         let vector_db = VectorDb::new(client.clone(), TEST_COLLECTION_NAME.to_string(), TEST_VECTOR_DIM)
             .expect("Failed to create VectorDb instance");
         vector_db.initialize_collection().await.expect("Collection initialization failed");

         let empty_docs: Vec<DocumentToUpsert> = vec![];
         let upsert_result = vector_db.upsert_documents(&empty_docs).await;
         assert!(upsert_result.is_ok(), "Upserting empty slice failed");

         // Verify point count is 0
         let count_response = client.count_points(TEST_COLLECTION_NAME, None, true).await.expect("Count request failed");
         assert_eq!(count_response.result.expect("Count result missing").count, 0, "Should have 0 points after upserting empty slice");

         let search_result = vector_db.search(vec![0.1, 0.2, 0.3, 0.4], 1).await;
         assert!(search_result.is_ok(), "Search after empty upsert failed");
         assert!(search_result.unwrap().is_empty(), "Search should return empty results after empty upsert");
     }
}

pub fn add(left: u64, right: u64) -> u64 {
    left + right
}

use comrak::{markdown_to_html, ComrakOptions, nodes::{AstNode, NodeValue}};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use fastembed::{TextEmbedding, InitOptions, EmbeddingModel, Error as FastEmbedError};
use log;
use serde::{Deserialize, Serialize};
use qdrant_client::qdrant::{PointStruct, Value, ScoredPoint, CreateCollection, Distance, VectorParams, VectorsConfig, SearchPoints};
use serde_json::json;
use uuid::Uuid;
use std::env;
use std::sync::Arc;

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

/// A simple in-memory index mapping file paths to their plain text content.
pub type SimpleDocumentIndex = HashMap<String, String>;

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

/// Payload to be stored in Qdrant along with the vector.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DocumentPayload {
    pub file_path: String,
    // Add other metadata fields if needed in the future
    // e.g., title: Option<String>, created_at: i64,
}

/// Represents a document ready to be upserted into Qdrant.
#[derive(Debug, Clone)]
pub struct DocumentToUpsert {
    pub file_path: String,
    pub vector: Vec<f32>,
    // Text content might be useful here too for context, but payload only needs file_path for now
    // pub text_content: String,
}

/// Represents a connection to the Qdrant vector database.
pub struct VectorDb {
    // ... existing code ...
}

impl VectorDb {
    // ... new() method ...

    // ... initialize_collection() method ...

    /// Upserts a batch of documents into the Qdrant collection.
    /// Generates unique IDs for each document.
    pub async fn upsert_documents(&self, documents: &[DocumentToUpsert]) -> Result<(), Error> {
        if documents.is_empty() {
            log::info!("No documents provided for upsert.");
            return Ok(());
        }

        let points: Vec<PointStruct> = documents
            .iter()
            .filter_map(|doc| {
                // Create the payload
                let payload_struct = DocumentPayload {
                    file_path: doc.file_path.clone(),
                };
                // Convert the payload struct to Qdrant's Payload format (HashMap<String, Value>)
                let payload: Option<Payload> = match serde_json::to_value(payload_struct) {
                    Ok(serde_json::Value::Object(map)) => Some(
                        map.into_iter()
                            .map(|(k, v)| (k, v.into())) // Convert serde_json::Value to qdrant_client::qdrant::Value
                            .collect::<HashMap<String, Value>>()
                            .into(), // Convert HashMap to Payload
                    ),
                    Ok(_) => {
                        log::error!("Serialized payload is not a JSON object for file: {}", doc.file_path);
                        None
                    }
                    Err(e) => {
                        log::error!("Failed to serialize payload for file {}: {}", doc.file_path, e);
                        None
                    }
                };

                // Only create a point if payload serialization was successful
                payload.map(|p| {
                    PointStruct::new(
                        Uuid::new_v4().to_string(), // Generate a unique ID for each point
                        doc.vector.clone(),         // The embedding vector
                        p,                          // The associated metadata
                    )
                })
            })
            .collect();

        if points.is_empty() {
            if documents.is_empty() {
                 log::info!("No documents provided for upsert (initial check).");
                 return Ok(()); // No documents to process
            } else {
                // This case happens if all payload serializations failed or documents list was non-empty but resulted in zero points
                log::error!("Failed to create any points for upsert, possibly due to payload serialization errors for all {} input documents.", documents.len());
                return Err(anyhow::anyhow!("Failed to create points for upsert"));
            }
        }

        log::info!("Upserting {} points to collection '{}' (from {} input documents)...", points.len(), self.collection_name, documents.len());

        // Perform the upsert operation
        // Setting wait to Some(true) ensures the operation completes before returning.
        // Note: Depending on qdrant-client version/API, you might need upsert_points or upsert_points_blocking
        match self.client
            .upsert_points_blocking(&self.collection_name, None, points.clone(), Some(true))
            .await {
                Ok(response) => {
                    // Optional: Check response status
                    if let Some(op_info) = response.result {
                        if op_info.status == qdrant_client::qdrant::UpdateStatus::Completed as i32 {
                            log::info!("Successfully upserted {} points. Operation ID: {}, Status: Completed", points.len(), op_info.operation_id);
                        } else {
                            log::warn!("Upsert operation finished with status: {}. Operation ID: {}", op_info.status, op_info.operation_id);
                            // Depending on the status, you might want to return an error or retry
                        }
                    } else {
                        log::warn!("Upsert operation response did not contain result info.");
                    }
                    Ok(())
                },
                Err(e) => {
                    log::error!("Failed to upsert points: {}", e);
                    // Consider adding more context to the error, e.g., the first few point IDs if possible
                    Err(e.into()) // Convert the client error into anyhow::Error
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
    /// A Result containing a vector of `ScoredPoint` structs on success, or an error.
    /// Each `ScoredPoint` includes the document ID, score, and payload.
    pub async fn search(&self, query_vector: Vec<f32>, limit: usize) -> Result<Vec<ScoredPoint>, Error> {
        if query_vector.is_empty() {
            log::warn!("Search query vector is empty.");
            // Return empty results or an error depending on desired behavior
             return Ok(Vec::new());
            // Alternatively: return Err(anyhow::anyhow!("Query vector cannot be empty"));
        }
        if limit == 0 {
            log::warn!("Search limit is 0. Returning empty results.");
            return Ok(Vec::new());
        }


        let search_request = SearchPoints {
            collection_name: self.collection_name.clone(),
            vector: query_vector,
            limit: limit as u64, // Qdrant API expects u64 for limit
            with_payload: Some(true.into()), // Include the payload in the results
            with_vector: Some(false.into()), // Usually, we don't need the vector itself in search results
            // filter: Option<Filter>, // TODO: Add filtering capabilities later if needed
            // score_threshold: Option<f32>, // Add a score threshold if needed
            ..Default::default()
        };

        log::info!("Performing search in collection '{}' with limit {}", self.collection_name, limit);

        let search_result = match self.client.search_points(&search_request).await {
             Ok(result) => result,
             Err(e) => {
                log::error!("Search points failed: {}", e);
                return Err(e.into());
             }
        };

        log::info!("Search completed. Found {} results.", search_result.result.len());

        // The result field directly contains Vec<ScoredPoint>
        Ok(search_result.result)
    }
}

// --- Add new test module below ---

#[cfg(test)]
mod vector_db_tests {
    use super::*; // Import items from the parent module (VectorDb, DocumentToUpsert, etc.)
    use testcontainers::runners::AsyncRunner; // Use AsyncRunner for async tests
    use testcontainers::ImageExt;
    use testcontainers_modules::qdrant::Qdrant; // Revert back to this use statement
    use tokio; // For async runtime
    use serial_test::serial; // To run tests serially
    use std::sync::Once; // To ensure container setup runs only once per process
    use qdrant_client::qdrant::Distance; // Import Distance for assertion

    // Static variable to ensure Docker setup runs only once
    static INIT: Once = Once::new();
    // Using Option<u16> inside OnceCell or Mutex would be safer than static mut
    static mut QDRANT_PORT: Option<u16> = None; // Store the mapped port

    // Helper function to set up the Qdrant container and environment variable
    // This will run only once for all tests in this module due to `Once` and `serial`.
    // Returns the gRPC port
    async fn setup_qdrant_container() -> u16 {
        let mut port_to_return = 0;
        // Use INIT.call_once to ensure the setup code runs only once across all threads
        INIT.call_once(|| {
             // This block runs only once
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                println!("Initializing Qdrant container for tests...");
                let qdrant_image = Qdrant::default().with_wait_for(testcontainers::core::WaitFor::message_on_stderr("gRPC server starting on 0.0.0.0:6334"));
                let node = qdrant_image.start().await.expect("Failed to start Qdrant container");
                let grpc_port = node.get_host_port_ipv4(6334).await.expect("Failed to get Qdrant gRPC port");

                let qdrant_url = format!("http://localhost:{}", grpc_port);
                std::env::set_var("QDRANT_URL", &qdrant_url);
                println!("Qdrant container started. QDRANT_URL set to: {}", qdrant_url);

                unsafe {
                    QDRANT_PORT = Some(grpc_port);
                }
                port_to_return = grpc_port;
            });
        });

        // If call_once ran, port_to_return will have the value.
        // If call_once didn't run (because it already ran), retrieve from static mut.
        unsafe {
            QDRANT_PORT.expect("QDRANT_PORT should be set after INIT.call_once")
        }
    }

    // Basic test: Ensure connection and collection initialization works
    #[tokio::test]
    #[serial] // Run this test serially because it modifies the environment (env var) and uses Docker
    async fn test_vector_db_new_and_initialize() {
        // Ensure container is running and env var is set
        setup_qdrant_container().await;

        // Create VectorDb instance
        let vector_db = VectorDb::new().await.expect("Failed to create VectorDb instance");

        // Initialize collection (should create it the first time)
        vector_db.initialize_collection().await.expect("Failed to initialize collection (first time)");

        // Try initializing again (should detect existing collection and verify params)
        vector_db.initialize_collection().await.expect("Failed to initialize collection (second time)");

        // Verify collection info using the client directly
        let client = vector_db.client.clone(); // Get underlying client
        let info = client.collection_info(&vector_db.collection_name).await.expect("Failed to get collection info");
        assert!(info.result.is_some(), "Collection info result should not be None");
        let collection_description = info.result.unwrap();
        assert_eq!(collection_description.status, qdrant_client::qdrant::CollectionStatus::Green as i32, "Collection status should be Green");

        let config = collection_description.vectors_config.expect("Vectors config should exist").config.expect("Config variant should exist");
         match config {
            qdrant_client::qdrant::vectors_config::Config::Params(params) => {
                 assert_eq!(params.size, 384, "Vector size should match");
                 assert_eq!(Distance::from_i32(params.distance).unwrap_or(Distance::UnknownDistance), Distance::Cosine, "Distance should match");
             }
             _ => panic!("Expected unnamed vectors config (Params)"),
         }
         println!("test_vector_db_new_and_initialize passed.");
    }

    // Test upserting documents and searching for them
    #[tokio::test]
    #[serial] // Also runs serially after the initialize test
    async fn test_vector_db_upsert_and_search() {
        // Ensure container is running and env var is set
        setup_qdrant_container().await;

        // Create VectorDb instance
        let vector_db = VectorDb::new().await.expect("Failed to create VectorDb instance");

        // Ensure collection exists (it should from the previous test or get created here)
        vector_db.initialize_collection().await.expect("Failed to initialize collection");

        // Clear the collection before testing upsert/search to ensure clean state
        // Note: delete_collection is easier than deleting all points if tests might run independently
        // Or, use different collection names per test if needed.
        // For simplicity here, we assume serial execution keeps state, but clearing is safer.
        // Let's try clearing points instead of deleting the collection, as recreation takes time.
        // vector_db.client.delete_points(&vector_db.collection_name, &PointsSelector { points_selector_one_of: Some(PointSelectorOneOf::Filter(Filter::all())) }, None).await.expect("Failed to clear points before test");
        // Simpler approach for now: hope the serial execution is enough. If flakes occur, implement clearing.


        // --- Prepare test data ---
        let doc1_path = "path/to/doc1.md".to_string();
        let mut vec1 = vec![0.1; 384]; // Create a 384-dim vector
        vec1[0] = 0.9; // Make it distinct

        let doc2_path = "another/path/doc2.md".to_string();
        let mut vec2 = vec![0.2; 384];
        vec2[1] = 0.8;

        let documents_to_upsert = vec![
            DocumentToUpsert { file_path: doc1_path.clone(), vector: vec1.clone() },
            DocumentToUpsert { file_path: doc2_path.clone(), vector: vec2.clone() },
        ];

        // --- Test Upsert ---
        vector_db.upsert_documents(&documents_to_upsert).await.expect("Failed to upsert documents");

        // Optional: Add a small delay or check point count if needed, though wait=true should suffice
        // tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        // Let's check the count to be sure
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await; // Give Qdrant a moment
        let count_response = vector_db.client.count_points(&vector_db.collection_name, None, Some(true)).await.expect("Failed to count points");
        assert_eq!(count_response.result.map(|r| r.count).unwrap_or(0), 2, "Should have 2 points after upsert");


        // --- Test Search (Find doc1) ---
        let search_limit = 1;
        let search_results_1 = vector_db.search(vec1.clone(), search_limit).await.expect("Search failed for vec1");

        assert_eq!(search_results_1.len(), search_limit, "Should find 1 result for vec1");
        let top_result_1 = &search_results_1[0];

        println!("Search 1 Score: {}", top_result_1.score);
        assert!(top_result_1.score > 0.99, "Score for exact match should be very close to 1.0 (Cosine)"); // Cosine similarity

        let payload_1 = top_result_1.payload.clone().expect("Payload should exist for search result 1");
        let file_path_1_val = payload_1.get("file_path").expect("Payload should contain file_path");
        // Payload values are qdrant_client::qdrant::Value, need conversion
        let file_path_1 = match &file_path_1_val.kind {
             Some(qdrant_client::qdrant::value::Kind::StringValue(s)) => s.clone(),
             _ => panic!("file_path in payload is not a string value"),
        };
        assert_eq!(file_path_1, doc1_path, "Found document should have the correct file path for vec1");


        // --- Test Search (Find doc2) ---
         let search_results_2 = vector_db.search(vec2.clone(), search_limit).await.expect("Search failed for vec2");

        assert_eq!(search_results_2.len(), search_limit, "Should find 1 result for vec2");
        let top_result_2 = &search_results_2[0];

        println!("Search 2 Score: {}", top_result_2.score);
         assert!(top_result_2.score > 0.99, "Score for exact match should be very close to 1.0 (Cosine)");

        let payload_2 = top_result_2.payload.clone().expect("Payload should exist for search result 2");
        let file_path_2_val = payload_2.get("file_path").expect("Payload should contain file_path");
        let file_path_2 = match &file_path_2_val.kind {
             Some(qdrant_client::qdrant::value::Kind::StringValue(s)) => s.clone(),
             _ => panic!("file_path in payload is not a string value"),
        };
        assert_eq!(file_path_2, doc2_path, "Found document should have the correct file path for vec2");


        // --- Test Search (Find closest to something in between) ---
        let mut vec_between = vec![0.15; 384]; // Mix of vec1 and vec2 elements
        vec_between[0] = 0.5; // Closer to vec1 bias
        vec_between[1] = 0.4; // Closer to vec2 bias

        let search_results_3 = vector_db.search(vec_between.clone(), search_limit).await.expect("Search failed for vec_between");
        assert_eq!(search_results_3.len(), search_limit, "Should find 1 result for vec_between");
        let top_result_3 = &search_results_3[0];
         println!("Search 3 (between) Score: {}", top_result_3.score);
         // Depending on the exact vectors, the closest might be vec1 or vec2.
         // We are just checking if the search returns *something* sensible here.
         let payload_3 = top_result_3.payload.clone().expect("Payload should exist for search result 3");
         let file_path_3_val = payload_3.get("file_path").expect("file_path should exist");
         let file_path_3 = match &file_path_3_val.kind {
             Some(qdrant_client::qdrant::value::Kind::StringValue(s)) => s.clone(),
             _ => panic!("file_path in payload is not a string value"),
         };
         println!("Search 3 found: {}", file_path_3);
         assert!(file_path_3 == doc1_path || file_path_3 == doc2_path, "Search result for 'between' vector should be one of the inserted docs");


         println!("test_vector_db_upsert_and_search passed.");
    }
}

use mc_mcp_application::{ReferenceService, ReferenceServiceImpl};
use mc_mcp_domain::reference::SearchQuery;
use mc_mcp_infrastructure::{
    EmbeddingGenerator, // Use the concrete struct
    VectorDb,         // Use the concrete struct
    EmbeddingModel,
};
use std::sync::Arc;
use std::path::PathBuf;
use testcontainers::{ContainerAsync, GenericImage, runners::AsyncRunner, ImageExt};
use testcontainers::core::{ContainerPort, WaitFor};
use qdrant_client::Qdrant;
use tempfile::tempdir;
use std::fs::{self, File};
use std::io::Write;
use anyhow::Result;

const QDRANT_GRPC_PORT: u16 = 6334;
const TEST_COLLECTION_NAME: &str = "test_collection";
const QDRANT_IMAGE: &str = "qdrant/qdrant";
const QDRANT_TAG: &str = "latest";
// Use a small, fast embedding model for testing
const TEST_EMBEDDING_MODEL: EmbeddingModel = EmbeddingModel::AllMiniLML6V2;
const EMBEDDING_DIM: u64 = 384; // Dimension for AllMiniLML6V2

// Helper function to set up Qdrant container and service
async fn setup_test_environment() -> Result<(
    ReferenceServiceImpl,
    ContainerAsync<GenericImage>,
    PathBuf,                // Path to temporary test docs directory
    tempfile::TempDir,      // TempDirを返す
)> {
    let qdrant_image = GenericImage::new(QDRANT_IMAGE, QDRANT_TAG)
        .with_exposed_port(ContainerPort::Tcp(QDRANT_GRPC_PORT))
        .with_exposed_port(ContainerPort::Tcp(6333))
        .with_wait_for(WaitFor::message_on_stdout("Actix runtime found"))
        .with_startup_timeout(std::time::Duration::from_secs(120));

    let qdrant_container = qdrant_image.start().await?;

    let qdrant_grpc_port = qdrant_container.get_host_port_ipv4(QDRANT_GRPC_PORT).await?;
    let qdrant_url = format!("http://localhost:{}", qdrant_grpc_port);

    println!("Qdrant running at: {}", qdrant_url);

    // Initialize infrastructure components
    let embedder = Arc::new(EmbeddingGenerator::new(TEST_EMBEDDING_MODEL)?);
    let qdrant_client = Qdrant::from_url(&qdrant_url).build()?;

    // QdrantのgRPC疎通をリトライで確認
    wait_for_qdrant_ready(&qdrant_url, std::time::Duration::from_secs(30)).await?;

    // VectorDb requires client, collection name, and vector size
    let vector_db = Arc::new(VectorDb::new(
        qdrant_client,
        TEST_COLLECTION_NAME.to_string(),
        EMBEDDING_DIM,
    )?);

    // Ensure collection exists (or create it)
    vector_db.initialize_collection().await?;
    println!("Qdrant collection '{}' initialized.", TEST_COLLECTION_NAME);

    let service = ReferenceServiceImpl::new(embedder, vector_db);

    // Create a temporary directory for test documents
    let temp_dir = tempdir()?;
    let docs_path = temp_dir.path().to_path_buf();

    // You might want to create some dummy markdown files here
    // e.g., create_dummy_docs(&docs_path)?;

    Ok((service, qdrant_container, docs_path, temp_dir))
}

// Helper to create dummy markdown files
fn create_dummy_docs(docs_path: &PathBuf) -> Result<()> {
    let doc1_path = docs_path.join("doc1.md");
    let mut file1 = File::create(doc1_path)?;
    writeln!(file1, "# Document 1\n\nThis is the first test document.")?;

    let doc2_path = docs_path.join("doc2.md");
    let mut file2 = File::create(doc2_path)?;
    writeln!(file2, "## Another Document\n\nContent for the second file.")?;

    let sub_dir = docs_path.join("subdir");
    fs::create_dir(&sub_dir)?;
    let doc3_path = sub_dir.join("doc3.md");
    let mut file3 = File::create(doc3_path)?;
    writeln!(file3, "### Deeper Doc\n\nA document in a subdirectory.")?;

    Ok(())
}


#[tokio::test]
#[ignore] // Ignore by default as it requires Docker and downloads models
async fn test_integration_index_and_search_happy_path() -> Result<()> {
    let (service, _container, docs_path, _temp_dir) = setup_test_environment().await?;
    create_dummy_docs(&docs_path)?;

    // 1. Index documents
    let index_result = service.index_documents(Some(docs_path.clone())).await;
    println!("Indexing result: {:?}", index_result);
    assert!(index_result.is_ok(), "Indexing failed");

    // Give Qdrant a moment to process potential async indexing
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;


    // 2. Search for content
    let query = SearchQuery {
        text: "first test document".to_string(),
        limit: Some(5),
    };
    let search_result = service.search_documents(query, None).await;
    println!("Search result: {:?}", search_result);
    assert!(search_result.is_ok(), "Search failed");

    let results = search_result.unwrap();
    assert!(!results.is_empty(), "Search should return results");

    // Check if the most relevant result is doc1.md
    assert!(results[0].file_path.contains("doc1.md"));
    // Add more specific assertions about score or content if needed

    Ok(())
}

#[tokio::test]
#[ignore]
async fn test_integration_index_no_documents() -> Result<()> {
    let (service, _container, docs_path, _temp_dir) = setup_test_environment().await?;
    // No documents created in docs_path

    let index_result = service.index_documents(Some(docs_path.clone())).await;
    assert!(index_result.is_ok()); // Should succeed even if no docs found (logs warning)

    // Search should return empty results
    let query = SearchQuery { text: "anything".to_string(), limit: Some(5) };
    let search_result = service.search_documents(query, None).await?;
    assert!(search_result.is_empty());

    Ok(())
}

#[tokio::test]
#[ignore]
async fn test_integration_search_no_results() -> Result<()> {
    let (service, _container, docs_path, _temp_dir) = setup_test_environment().await?;
    create_dummy_docs(&docs_path)?;

    // Index documents first
    service.index_documents(Some(docs_path.clone())).await?;
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

    // Search for text unlikely to be found
    let query = SearchQuery { text: "xyzzy irrelevant query".to_string(), limit: Some(5) };
    let search_result = service.search_documents(query, Some(0.1)).await?;
    assert!(search_result.is_empty()); // Expect no results

    Ok(())
}

// Error case testing is harder with direct infrastructure use.
// Potential errors:
// - Qdrant connection issues (covered partly by setup)
// - Embedding model loading issues (covered partly by setup)
// - Document loading errors (can test by providing invalid path)
// - Qdrant errors during upsert/search (harder to simulate reliably without mocks)

#[tokio::test]
#[ignore]
async fn test_integration_index_invalid_path() -> Result<()> {
     let (service, _container, _docs_path, _temp_dir) = setup_test_environment().await?;
     let invalid_path = PathBuf::from("/path/that/does/not/exist");

     let index_result = service.index_documents(Some(invalid_path)).await;
     assert!(index_result.is_err()); // Expect an error because path doesn't exist

     Ok(())
}

// QdrantのgRPC疎通をリトライで確認する関数
async fn wait_for_qdrant_ready(url: &str, timeout: std::time::Duration) -> Result<()> {
    use std::time::Instant;
    let start = Instant::now();
    loop {
        let client = Qdrant::from_url(url).build();
        if let Ok(client) = client {
            if client.health_check().await.is_ok() {
                break Ok(());
            }
        }
        if start.elapsed() > timeout {
            break Err(anyhow::anyhow!("Qdrant did not become ready in time"));
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

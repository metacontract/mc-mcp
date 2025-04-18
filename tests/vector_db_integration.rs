use anyhow::Result;
use mc_mcp::infrastructure::vector_db::{DocumentToUpsert, VectorDb};
use mc_mcp::qdrant_client::Qdrant;
use mc_mcp::VectorRepository;
use std::time::Duration;
use tokio;
use uuid::Uuid;

// Pseudo Qdrant URL for testing
async fn get_qdrant_url() -> anyhow::Result<String> {
    Ok("http://localhost:6334".to_string())
}

#[tokio::test]
async fn test_vector_db_new_and_initialize() -> Result<()> {
    let qdrant_url = get_qdrant_url().await?;
    let client = Qdrant::from_url(&qdrant_url)
        .build()
        .expect("Failed to create Qdrant client");
    let vector_db = VectorDb::new(Box::new(client), "test_collection_init".to_string(), 3)?;
    vector_db.initialize_collection().await?;
    vector_db.initialize_collection().await?;
    Ok(())
}

#[tokio::test]
async fn test_vector_db_upsert_and_search() -> Result<()> {
    let qdrant_url = get_qdrant_url().await?;
    let client = Qdrant::from_url(&qdrant_url).build()?;
    let collection_name = format!("test_coll_{}", Uuid::new_v4());
    let vector_size: u64 = 3;
    let vector_db = VectorDb::new(Box::new(client), collection_name.clone(), vector_size)?;
    vector_db.initialize_collection().await?;
    let docs_to_upsert = vec![
        DocumentToUpsert {
            file_path: "file1.md".to_string(),
            vector: vec![0.1, 0.2, 0.7],
            source: Some("source_A".to_string()),
            content_chunk: "This is chunk 1 from source A.".to_string(),
            metadata: Some(serde_json::json!({ "section": "intro" })),
        },
        DocumentToUpsert {
            file_path: "file2.md".to_string(),
            vector: vec![0.8, 0.1, 0.1],
            source: Some("source_B".to_string()),
            content_chunk: "This is chunk 1 from source B.".to_string(),
            metadata: None,
        },
        DocumentToUpsert {
            file_path: "file1.md".to_string(),
            vector: vec![0.2, 0.3, 0.5],
            source: Some("source_A".to_string()),
            content_chunk: "This is chunk 2 from source A.".to_string(),
            metadata: Some(serde_json::json!({ "section": "details" })),
        },
    ];
    vector_db.upsert_documents(&docs_to_upsert).await?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let query_vector = vec![0.15, 0.25, 0.6];
    let results = vector_db.search(query_vector.clone(), 5, Some(0.5)).await?;
    println!("Search Results: {:?}", results);
    assert!(!results.is_empty(), "Search should return results");
    assert_eq!(results.len(), 2, "Expected 2 results above threshold 0.5");
    let top_result = &results[0];
    assert!(top_result.score > 0.5, "Score should be above threshold");
    assert_eq!(top_result.file_path, "file1.md");
    assert_eq!(top_result.source, Some("source_A".to_string()));
    assert_eq!(top_result.content_chunk, "This is chunk 1 from source A.");
    assert_eq!(
        top_result.metadata,
        Some(serde_json::json!({ "section": "intro" }))
    );
    let second_result = &results[1];
    assert!(second_result.score > 0.5);
    assert_eq!(second_result.file_path, "file1.md");
    assert_eq!(second_result.source, Some("source_A".to_string()));
    assert_eq!(
        second_result.content_chunk,
        "This is chunk 2 from source A."
    );
    assert_eq!(
        second_result.metadata,
        Some(serde_json::json!({ "section": "details" }))
    );
    let query_vector_b = vec![0.7, 0.15, 0.15];
    let search_result_b = vector_db
        .search(query_vector_b.clone(), 5, Some(0.5))
        .await?;
    assert_eq!(search_result_b.len(), 2, "Expected 2 results for query B");
    assert_eq!(search_result_b[0].file_path, "file2.md");
    assert_eq!(search_result_b[0].source, Some("source_B".to_string()));
    assert_eq!(
        search_result_b[0].content_chunk,
        "This is chunk 1 from source B."
    );
    assert!(search_result_b[0].metadata.is_none());
    Ok(())
}

#[tokio::test]
async fn test_vector_db_new_invalid_params() -> Result<()> {
    let qdrant_url = get_qdrant_url().await?;
    let client1 = Qdrant::from_url(&qdrant_url).build()?;
    assert!(VectorDb::new(Box::new(client1), "".to_string(), 3).is_err());
    let client2 = Qdrant::from_url(&qdrant_url).build()?;
    assert!(VectorDb::new(Box::new(client2), "test".to_string(), 0).is_err());
    Ok(())
}

#[tokio::test]
async fn test_vector_db_search_wrong_dimension() -> Result<()> {
    let qdrant_url = get_qdrant_url().await?;
    let client = Qdrant::from_url(&qdrant_url).build()?;
    let collection_name = format!("test_coll_dim_{}", Uuid::new_v4());
    let vector_db = VectorDb::new(Box::new(client), collection_name, 3)?;
    vector_db.initialize_collection().await?;
    let query_vector = vec![0.1, 0.2];
    let search_result = vector_db.search(query_vector, 5, None).await;
    assert!(search_result.is_err());
    assert!(search_result
        .unwrap_err()
        .to_string()
        .contains("Query vector dimension"));
    Ok(())
}

#[tokio::test]
async fn test_vector_db_upsert_empty() -> Result<()> {
    let qdrant_url = get_qdrant_url().await?;
    let client = Qdrant::from_url(&qdrant_url).build()?;
    let collection_name = format!("test_coll_empty_{}", Uuid::new_v4());
    let vector_db = VectorDb::new(Box::new(client), collection_name, 3)?;
    vector_db.initialize_collection().await?;
    let empty_docs: Vec<DocumentToUpsert> = vec![];
    vector_db.upsert_documents(&empty_docs).await?;
    Ok(())
}

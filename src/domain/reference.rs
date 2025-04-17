use serde::{Deserialize, Serialize};
use anyhow::Result;
use crate::config::DocumentSource;
use std::sync::Arc;
use futures::future::BoxFuture;
use mockall::automock;
use qdrant_client::qdrant::{PointStruct, ScoredPoint};

// Represents a query for searching documents
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchQuery {
    pub text: String,
    pub limit: Option<usize>,
    pub sources: Option<Vec<String>>,
    // Add filter capabilities later if needed
    // pub filter: Option<serde_json::Value>,
}

// Represents a relevant fragment of a document found during search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentFragment {
    pub text: String, // The actual text fragment
    // pub metadata: Option<serde_json::Value>, // Optional metadata about the fragment
}

// Represents a search result, linking back to a file path
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchResult {
    pub file_path: String, // Path to the source document
    pub score: f32,        // Similarity score
    pub source: Option<String>, // Source identifier (e.g., "mc-docs", "local-project") - Made optional
    pub content_chunk: String, // The actual text chunk that matched
    pub metadata: Option<serde_json::Value>, // Optional metadata associated with the chunk
    // pub fragment: Option<DocumentFragment>, // Removed/Replaced by content_chunk and metadata
}

// Define the interface for reference-related operations
#[async_trait::async_trait]
pub trait ReferenceService: Send + Sync + 'static {
    // Comment out the unused method definition
    // async fn index_documents(&self, docs_path: Option<PathBuf>) -> Result<()>;
    async fn index_sources(&self, sources: &[DocumentSource]) -> Result<()>;
    async fn search_documents(&self, query: SearchQuery, score_threshold: Option<f32>) -> Result<Vec<SearchResult>>;
    fn search(&self, collection_name: String, vector: Vec<f32>, limit: u64) -> BoxFuture<Result<Vec<ScoredPoint>, String>>;
    fn upsert(&self, collection_name: String, points: Vec<PointStruct>) -> BoxFuture<Result<(), String>>;
}

pub struct EmbeddingResult {
    pub vectors: Vec<Vec<f32>>,
}

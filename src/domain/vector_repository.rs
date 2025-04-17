// NOTE: Define the VectorRepository trait here
use async_trait::async_trait;
use anyhow::Result;

// Using infrastructure type directly for now. Consider domain-specific type later.
use crate::infrastructure::vector_db::DocumentToUpsert; // Updated path if needed
// Use domain::SearchResult for the search return type
use crate::domain::reference::SearchResult;

#[async_trait]
pub trait VectorRepository: Send + Sync {
    /// Upserts multiple documents into the vector store.
    /// `documents` should contain necessary payload data (source, content_chunk, metadata).
    async fn upsert_documents(&self, documents: &[DocumentToUpsert]) -> Result<()>;

    /// Searches the vector store based on a query vector.
    /// Returns domain-specific SearchResult directly.
    async fn search(&self, query_vector: Vec<f32>, limit: usize, score_threshold: Option<f32>) -> Result<Vec<SearchResult>>;

    // Optional: Add other methods like initialize_collection if needed at domain/app level
    // async fn initialize_collection(&self) -> Result<()>;
}

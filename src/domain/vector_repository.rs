// NOTE: Define the VectorRepository trait here
use async_trait::async_trait;
use anyhow::Result;

// Assuming DocumentToUpsert lives in infrastructure::embedding
// We might want domain-specific types later, but for now, use infra type
use crate::infrastructure::embedding::DocumentToUpsert;
// Assuming ScoredPoint comes from the qdrant_client re-exported by infrastructure
use crate::infrastructure::qdrant_client::qdrant::ScoredPoint;

#[async_trait]
pub trait VectorRepository: Send + Sync {
    /// Upserts multiple documents into the vector store.
    async fn upsert_documents(&self, documents: &[DocumentToUpsert]) -> Result<()>;

    /// Searches the vector store based on a query vector.
    /// Should return ScoredPoint to align with application layer usage.
    async fn search(&self, query_vector: Vec<f32>, limit: usize, score_threshold: Option<f32>) -> Result<Vec<ScoredPoint>>;

    // Optional: Add other methods like initialize_collection if needed at domain/app level
    // async fn initialize_collection(&self) -> Result<()>;
}

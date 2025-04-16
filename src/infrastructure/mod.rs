pub mod file_system;
pub mod markdown;
pub mod embedding;
pub mod vector_db;

// Re-export key types for easier access from application layer
pub use file_system::{load_documents, SimpleDocumentIndex};
// pub use markdown::parse_markdown_to_text; // Keep commented out as it's unused
pub use embedding::{EmbeddingGenerator, DocumentToUpsert}; // Remove EmbeddingModel from here
pub use vector_db::{VectorDb, DocumentPayload, qdrant_client};

// Re-export EmbeddingModel directly from the dependency
pub use fastembed::EmbeddingModel;

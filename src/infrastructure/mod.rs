pub mod file_system;
pub mod markdown;
pub mod embedding;
pub mod vector_db;

// Re-export key types for easier access from application layer
// pub use markdown::parse_markdown_to_text; // Keep commented out as it's unused
pub use embedding::EmbeddingGenerator; // Remove EmbeddingModel from here

// Re-export EmbeddingModel directly from the dependency
pub use fastembed::EmbeddingModel;

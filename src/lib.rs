pub mod application;
pub mod config;
/// Declare modules and make them public
pub mod domain;
pub mod infrastructure;

/// Re-export necessary items for main.rs and tests
pub use application::reference_service::ReferenceServiceImpl;
pub use config::load_config;
pub use config::DocumentSource; // Export DocumentSource as it's used in ReferenceService trait sig and main.rs
pub use domain::reference::ReferenceService;
pub use domain::vector_repository::VectorRepository;
pub use fastembed::EmbeddingModel;
pub use infrastructure::embedding::EmbeddingGenerator;
pub use infrastructure::file_system;
pub use infrastructure::vector_db::{qdrant_client, DocumentToUpsert, VectorDb}; // Make DocumentToUpsert public for tests // Make file_system public for load_prebuilt_index

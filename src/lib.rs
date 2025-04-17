/// Declare modules and make them public
pub mod domain;
pub mod config;
pub mod application;
pub mod infrastructure;

/// Re-export necessary items for main.rs and tests
pub use application::reference_service::ReferenceServiceImpl;
pub use domain::reference::ReferenceService;
pub use infrastructure::embedding::EmbeddingGenerator;
pub use fastembed::EmbeddingModel;
pub use infrastructure::vector_db::{VectorDb, DocumentToUpsert, qdrant_client}; // Make DocumentToUpsert public for tests
pub use domain::vector_repository::VectorRepository;
pub use config::load_config;
pub use config::DocumentSource; // Export DocumentSource as it's used in ReferenceService trait sig and main.rs
pub use infrastructure::file_system; // Make file_system public for load_prebuilt_index

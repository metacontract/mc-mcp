use fastembed::{EmbeddingModel, Error as FastEmbedError, InitOptions, TextEmbedding};
use serde::{Deserialize, Serialize};

/// A struct responsible for generating text embeddings using a pre-initialized model.
pub struct EmbeddingGenerator {
    model: TextEmbedding,
}

impl EmbeddingGenerator {
    /// Creates a new EmbeddingGenerator, initializing the specified embedding model.
    ///
    /// # Arguments
    ///
    /// * `model_name` - The embedding model to use (e.g., EmbeddingModel::AllMiniLML6V2).
    /// * `cache_dir` - The cache directory for the embedding model (None for default).
    ///
    /// # Returns
    ///
    /// A Result containing the `EmbeddingGenerator` on success, or a `FastEmbedError` on failure.
    pub fn new(model_name: EmbeddingModel, cache_dir: Option<std::path::PathBuf>) -> Result<Self, FastEmbedError> {
        let mut opts = InitOptions::new(model_name);
        if let Some(dir) = cache_dir {
            opts = opts.with_cache_dir(dir);
        }
        let model = TextEmbedding::try_new(opts)?;
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

/// Represents a document chunk ready to be upserted into the vector database.
#[derive(Debug, Clone, Serialize, Deserialize)] // Added derive for potential use cases
pub struct DocumentToUpsert {
    pub file_path: String,
    pub vector: Vec<f32>,
    pub source: String, // 追加: ドキュメントソース情報
                        // Text content might be useful here too for context, but payload only needs file_path for now
                        // pub text_content: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    // Basic test to ensure initialization and embedding works
    // Note: This test might download model data on first run
    #[test]
    fn test_embedding_generator_init_and_embed() -> Result<(), FastEmbedError> {
        let model_name = EmbeddingModel::AllMiniLML6V2; // Use EmbeddingModel directly
        let generator = EmbeddingGenerator::new(model_name.clone(), None)?;

        let documents = vec!["This is a test document.", "Another document."];
        let embeddings = generator.generate_embeddings(&documents)?;

        assert_eq!(embeddings.len(), 2);
        // Check embedding dimension for the specific model (e.g., 384 for all-MiniLM-L6-v2)
        // This requires knowing the expected dimension.
        let expected_dim = TextEmbedding::list_supported_models()
            .iter()
            .find(|m| m.model == model_name)
            .map(|m| m.dim)
            .unwrap_or(0); // Handle case where model info might not be found

        if expected_dim > 0 {
            // Only assert if we found the dimension
            assert_eq!(embeddings[0].len(), expected_dim);
            assert_eq!(embeddings[1].len(), expected_dim);
        }

        Ok(())
    }
}

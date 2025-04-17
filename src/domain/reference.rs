use serde::{Deserialize, Serialize};

// Represents a query for searching documents
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchQuery {
    pub text: String,
    pub limit: Option<usize>,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub file_path: String, // Path to the source document
    pub score: f32,        // Similarity score
    pub source: Option<String>, // Source identifier (e.g., "mc-docs", "local-project") - Made optional
    pub content_chunk: String, // The actual text chunk that matched
    pub metadata: Option<serde_json::Value>, // Optional metadata associated with the chunk
    // pub fragment: Option<DocumentFragment>, // Removed/Replaced by content_chunk and metadata
}

// pub mod domain; // Remove this line, reference is a top-level module now

// Add domain types for reference tool
pub mod reference {
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
        // pub fragment: Option<DocumentFragment>, // Optional relevant fragment
    }
}

pub fn add(left: u64, right: u64) -> u64 {
    left + right
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        let result = add(2, 2);
        assert_eq!(result, 4);
    }
}

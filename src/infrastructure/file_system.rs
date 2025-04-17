use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use walkdir::WalkDir;
use super::markdown::parse_markdown_to_text; // Assuming markdown.rs exists
use serde_json;
use log::{debug, error, warn};

// Define module first
mod document_index {
    use std::collections::HashMap;
    pub type SimpleDocumentIndex = HashMap<String, (String, String)>; // (text, source)
}

pub use self::document_index::SimpleDocumentIndex;

/// Loads Markdown documents from a specified directory (or default) and creates a simple index.
///
/// Recursively searches for `.md` files in the given directory, reads them,
/// parses them to plain text, and stores them in a HashMap.
///
/// # Arguments
///
/// * `docs_path` - An optional PathBuf specifying the directory to load documents from.
///                 If None, defaults to "metacontract/mc/site/docs".
///
/// # Returns
///
/// A Result containing the `SimpleDocumentIndex` on success, or a String error message on failure.
pub fn load_documents(docs_path: Option<PathBuf>) -> Result<SimpleDocumentIndex, String> {
    let default_path = PathBuf::from("metacontract/mc/site/docs");
    let target_path = docs_path.unwrap_or(default_path);

    println!("Loading documents from: {:?}", target_path);

    if !target_path.is_dir() {
        return Err(format!("Specified path is not a directory: {:?}", target_path));
    }

    let mut index = SimpleDocumentIndex::new();

    for entry in WalkDir::new(&target_path)
        .into_iter()
        .filter_map(|e| e.ok()) // エラーになったエントリは無視
        .filter(|e| e.path().is_file() && e.path().extension().map_or(false, |ext| ext == "md"))
    {
        let path = entry.path();
        let path_str = path.to_string_lossy().to_string();

        match fs::read_to_string(path) {
            Ok(content) => {
                let text = parse_markdown_to_text(&content); // Use the function from markdown module
                index.insert(path_str, (text, "mc-docs".to_string())); // sourceは現状固定
            }
            Err(e) => {
                eprintln!("Failed to read file {}: {}", path_str, e);
            }
        }
    }

    if index.is_empty() {
       println!("Warning: No markdown files found or loaded from {:?}", target_path);
    }

    Ok(index)
}

/// Loads a prebuilt document index from a JSON file.
pub fn load_prebuilt_index(path: PathBuf) -> Result<SimpleDocumentIndex, String> {
    let file = std::fs::File::open(&path).map_err(|e| format!("Failed to open prebuilt index: {}", e))?;
    let raw_map: std::collections::HashMap<String, (String, String)> = serde_json::from_reader(file)
        .map_err(|e| format!("Failed to parse prebuilt index JSON: {}", e))?;
    Ok(raw_map)
}

/// Loads Markdown documents from multiple sources, each with its own source metadata.
pub fn load_documents_from_multiple_sources(sources: &[(PathBuf, String)]) -> Result<SimpleDocumentIndex, String> {
    let mut index = SimpleDocumentIndex::new();
    for (dir, source) in sources {
        if !dir.is_dir() {
            return Err(format!("Specified path is not a directory: {:?}", dir));
        }
        for entry in WalkDir::new(dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_file() && e.path().extension().map_or(false, |ext| ext == "md"))
        {
            let path = entry.path();
            let path_str = path.to_string_lossy().to_string();
            match fs::read_to_string(path) {
                Ok(content) => {
                    let text = parse_markdown_to_text(&content);
                    index.insert(path_str, (text, source.clone()));
                }
                Err(e) => {
                    eprintln!("Failed to read file {}: {}", path_str, e);
                }
            }
        }
    }
    Ok(index)
}

/// Loads Markdown documents from a single specified directory.
///
/// Recursively searches for `.md` files in the given directory, reads them,
/// and returns a map of file paths to their raw content.
///
/// # Arguments
///
/// * `dir_path` - The PathBuf specifying the directory to load documents from.
///
/// # Returns
///
/// A Result containing a HashMap where keys are file paths (String) and
/// values are the raw file content (String). Returns an error if the path is not a directory
/// or if there are issues reading files (individual file read errors are logged).
pub fn load_documents_from_source(dir_path: &PathBuf) -> Result<HashMap<String, String>> {
    debug!("Loading documents from single source: {:?}", dir_path);

    if !dir_path.is_dir() {
        return Err(anyhow::anyhow!("Specified path is not a directory: {:?}", dir_path));
    }

    let mut documents = HashMap::new();
    let mut read_errors = 0;

    for entry in WalkDir::new(dir_path)
        .into_iter()
        .filter_map(|e| e.ok()) // Ignore directory traversal errors
        .filter(|e| e.path().is_file() && e.path().extension().map_or(false, |ext| ext == "md"))
    {
        let path = entry.path();
        let path_str = path.to_string_lossy().to_string();

        match fs::read_to_string(path) {
            Ok(content) => {
                debug!("Successfully read: {}", path_str);
                // We return raw content here. Parsing/chunking happens later.
                documents.insert(path_str, content);
            }
            Err(e) => {
                error!("Failed to read file {}: {}", path_str, e);
                read_errors += 1;
            }
        }
    }

    if documents.is_empty() && read_errors == 0 {
        warn!("No markdown files found in {:?}", dir_path);
    } else if read_errors > 0 {
        warn!("Encountered {} errors while reading files from {:?}", read_errors, dir_path);
        // Decide if partial success is ok, or return an error?
        // For now, return successfully loaded documents, but log errors.
    }

    Ok(documents)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::io::Write;
    use tempfile::tempdir;
    use std::path::Path;

    // Mock parse_markdown_to_text for file_system tests
    mod markdown {
        pub fn parse_markdown_to_text(markdown: &str) -> String {
            markdown.replace("#", "").split_whitespace().collect::<Vec<&str>>().join(" ")
        }
    }
    use self::markdown::parse_markdown_to_text;

    #[test]
    fn test_load_documents_default_path_not_exists() {
        // Need to ensure the default path doesn't exist for this test
        if Path::new("metacontract/mc/site/docs").exists() {
           println!("Skipping test_load_documents_default_path_not_exists because default path exists.");
           return;
        }
        let result = load_documents(None);
        assert!(result.is_err());
    }

    #[test]
    fn test_load_documents_from_temp_dir() {
        let dir = tempdir().unwrap();
        let docs_path = dir.path().to_path_buf();

        fs::create_dir(docs_path.join("sub")).unwrap();
        let mut file1 = File::create(docs_path.join("file1.md")).unwrap();
        writeln!(file1, "# Title 1\nContent 1").unwrap();
        let mut file2 = File::create(docs_path.join("sub/file2.md")).unwrap();
        writeln!(file2, "* List item").unwrap();
        let mut file3 = File::create(docs_path.join("not_markdown.txt")).unwrap();
        writeln!(file3, "ignore me").unwrap();

        let index = load_documents(Some(docs_path.clone())).unwrap();

        assert_eq!(index.len(), 2);
        // Use the mocked parse_markdown_to_text result
        assert_eq!(index.get(&docs_path.join("file1.md").to_string_lossy().to_string()), Some(&("Title 1 Content 1".to_string(), "mc-docs".to_string())));
        assert_eq!(index.get(&docs_path.join("sub/file2.md").to_string_lossy().to_string()), Some(&("List item".to_string(), "mc-docs".to_string()))); // Mock parse result
        assert!(!index.contains_key(&docs_path.join("not_markdown.txt").to_string_lossy().to_string()));

        drop(file1);
        drop(file2);
        drop(file3);
        dir.close().unwrap();
    }

     #[test]
    fn test_load_documents_empty_dir() {
        let dir = tempdir().unwrap();
        let docs_path = dir.path().to_path_buf();

        let index = load_documents(Some(docs_path)).unwrap();
        assert!(index.is_empty());

        dir.close().unwrap();
    }

    #[test]
    fn test_load_documents_non_existent_dir() {
        let path = PathBuf::from("non_existent_dir_for_test");
        let result = load_documents(Some(path));
        assert!(result.is_err());
    }

    #[test]
    fn test_load_prebuilt_index_json() {
        use std::fs::File;
        use std::io::Write;
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let index_path = dir.path().join("prebuilt_index.json");
        // ダミーのインデックスデータ
        let dummy = serde_json::json!({
            "file1.md": ["Dummy content 1", "mc-docs"],
            "file2.md": ["Dummy content 2", "mc-docs"]
        });
        let mut file = File::create(&index_path).unwrap();
        write!(file, "{}", dummy.to_string()).unwrap();
        drop(file);
        // テスト対象関数（未実装）
        let result = load_prebuilt_index(index_path.clone());
        assert!(result.is_ok(), "Should load prebuilt index JSON");
        let index = result.unwrap();
        assert_eq!(index.len(), 2);
        assert_eq!(index.get("file1.md"), Some(&("Dummy content 1".to_string(), "mc-docs".to_string())));
        assert_eq!(index.get("file2.md"), Some(&("Dummy content 2".to_string(), "mc-docs".to_string())));
        dir.close().unwrap();
    }

    #[test]
    fn test_load_documents_with_additional_sources() {
        use std::fs::File;
        use std::io::Write;
        use tempfile::tempdir;
        use std::collections::HashMap;
        // メインdocsディレクトリ
        let main_dir = tempdir().unwrap();
        let main_md = main_dir.path().join("main.md");
        let mut f1 = File::create(&main_md).unwrap();
        writeln!(f1, "# Main doc").unwrap();
        drop(f1);
        // 追加ソース1
        let add1 = tempdir().unwrap();
        let add1_md = add1.path().join("add1.md");
        let mut f2 = File::create(&add1_md).unwrap();
        writeln!(f2, "# Add1 doc").unwrap();
        drop(f2);
        // 追加ソース2
        let add2 = tempdir().unwrap();
        let add2_md = add2.path().join("add2.md");
        let mut f3 = File::create(&add2_md).unwrap();
        writeln!(f3, "# Add2 doc").unwrap();
        drop(f3);
        // テスト対象: main_dir, [add1, add2] をまとめてインデックス化
        let sources = vec![
            (main_dir.path().to_path_buf(), "mc-docs".to_string()),
            (add1.path().to_path_buf(), "additional-1".to_string()),
            (add2.path().to_path_buf(), "additional-2".to_string()),
        ];
        let index = load_documents_from_multiple_sources(&sources).unwrap();
        // 期待: 3ファイル全てがインデックスされ、sourceメタデータも正しい
        let mut expected = HashMap::new();
        expected.insert(main_md.to_string_lossy().to_string(), ("Main doc".to_string(), "mc-docs".to_string()));
        expected.insert(add1_md.to_string_lossy().to_string(), ("Add1 doc".to_string(), "additional-1".to_string()));
        expected.insert(add2_md.to_string_lossy().to_string(), ("Add2 doc".to_string(), "additional-2".to_string()));
        assert_eq!(index, expected);
        main_dir.close().unwrap();
        add1.close().unwrap();
        add2.close().unwrap();
    }

    #[test]
    fn test_load_documents_from_source_success() {
        let dir = tempdir().unwrap();
        let source_path = dir.path().to_path_buf();

        fs::create_dir(source_path.join("subdir")).unwrap();
        let mut file1 = File::create(source_path.join("file1.md")).unwrap();
        writeln!(file1, "# Content 1").unwrap();
        let mut file2 = File::create(source_path.join("subdir/file2.md")).unwrap();
        writeln!(file2, "Content 2").unwrap();
        let mut file3 = File::create(source_path.join("other.txt")).unwrap();
        writeln!(file3, "Ignore").unwrap();

        let documents = load_documents_from_source(&source_path).unwrap();

        assert_eq!(documents.len(), 2);
        assert_eq!(documents.get(&source_path.join("file1.md").to_string_lossy().to_string()), Some(&"# Content 1\n".to_string()));
        assert_eq!(documents.get(&source_path.join("subdir/file2.md").to_string_lossy().to_string()), Some(&"Content 2\n".to_string()));
        assert!(!documents.contains_key(&source_path.join("other.txt").to_string_lossy().to_string()));
    }

    #[test]
    fn test_load_documents_from_source_empty() {
        let dir = tempdir().unwrap();
        let source_path = dir.path().to_path_buf();
        let documents = load_documents_from_source(&source_path).unwrap();
        assert!(documents.is_empty());
    }

    #[test]
    fn test_load_documents_from_source_not_a_directory() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("a_file.txt");
        File::create(&file_path).unwrap();
        let result = load_documents_from_source(&file_path);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not a directory"));
    }
}

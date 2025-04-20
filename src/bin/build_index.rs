use mc_mcp::infrastructure::embedding::EmbeddingGenerator;
use mc_mcp::infrastructure::file_system::load_documents_from_source;
use mc_mcp::infrastructure::vector_db::DocumentToUpsert;
use mc_mcp::infrastructure::EmbeddingModel;
use std::env;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

fn chunk_document(file_path: &str, content: &str) -> Vec<String> {
    // 既存のReferenceServiceImplと同じロジック
    content
        .split("\n\n")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

fn main() -> anyhow::Result<()> {
    // 引数: 入力ディレクトリ, 出力ファイル
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: build_index <input_docs_dir> <output_jsonl>");
        std::process::exit(1);
    }
    let input_dir = PathBuf::from(&args[1]);
    let output_path = PathBuf::from(&args[2]);

    // ドキュメント読み込み
    let documents = load_documents_from_source(&input_dir)?;
    let embedder = EmbeddingGenerator::new(EmbeddingModel::AllMiniLML6V2)?;
    let mut out = File::create(&output_path)?;
    let mut total_chunks = 0;

    for (file_path, content) in &documents {
        let chunks = chunk_document(file_path, content);
        if chunks.is_empty() {
            continue;
        }
        let chunk_slices: Vec<&str> = chunks.iter().map(AsRef::as_ref).collect();
        let embeddings = embedder.generate_embeddings(&chunk_slices)?;
        for (chunk_content, vector) in chunks.into_iter().zip(embeddings.into_iter()) {
            let doc = DocumentToUpsert {
                file_path: file_path.clone(),
                vector,
                source: Some("prebuilt".to_string()),
                content_chunk: chunk_content,
                metadata: None,
            };
            let json = serde_json::to_string(&doc)?;
            writeln!(out, "{}", json)?;
            total_chunks += 1;
        }
    }
    println!("Wrote {} chunks to {:?}", total_chunks, output_path);
    Ok(())
}

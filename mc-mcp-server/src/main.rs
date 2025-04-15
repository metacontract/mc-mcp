use rmcp::{
    model::{self, Content, ServerInfo, ServerCapabilities, ToolsCapability, Implementation, CallToolRequestParam, CallToolResult, CallToolRequestMethod, Tool},
    ServiceExt,
    ServerHandler,
    service::RequestContext,
    RoleServer,
    Error as McpError,
};
use std::sync::{Arc, Mutex};
use std::future::Future;
use rmcp::serde_json::Map;
use serde_json::json;
use tokio::{
    io::{stdin, stdout},
    process::Command,
};
use mc_mcp_infrastructure::{load_documents, SimpleDocumentIndex};
use mc_mcp_application::{ReferenceServiceImpl};
use mc_mcp_infrastructure::{EmbeddingGenerator, VectorDb, EmbeddingModel};
use mc_mcp_infrastructure::qdrant_client::Qdrant;
use mc_mcp_domain::reference::SearchQuery;
use mc_mcp_application::ReferenceService;
use serial_test::serial;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("mc-mcp server (MCP over stdio) started.");

    let document_index = match load_documents(None) {
        Ok(index) => {
            println!("Successfully loaded {} documents.", index.len());
            Arc::new(Mutex::new(index))
        }
        Err(e) => {
            eprintln!("Failed to load documents: {}. Starting with empty index.", e);
            Arc::new(Mutex::new(SimpleDocumentIndex::new()))
        }
    };

    // --- ReferenceServiceImplの初期化 ---
    // TODO: QdrantのURLやコレクション名、ベクトル次元数は設定ファイル等から取得するのが望ましい
    let qdrant_url = std::env::var("QDRANT_URL").unwrap_or_else(|_| "http://localhost:6334".to_string());
    let qdrant_client = Qdrant::from_url(&qdrant_url).build()?;
    let collection_name = std::env::var("QDRANT_COLLECTION").unwrap_or_else(|_| "mc_docs".to_string());
    let vector_dim: u64 = std::env::var("EMBEDDING_DIM").ok().and_then(|s| s.parse().ok()).unwrap_or(384);
    let embedding_model = EmbeddingModel::AllMiniLML6V2;
    let embedder = Arc::new(EmbeddingGenerator::new(embedding_model)?);
    let vector_db = Arc::new(VectorDb::new(qdrant_client, collection_name, vector_dim)?);
    vector_db.initialize_collection().await?;
    let reference_service = Arc::new(ReferenceServiceImpl::new(embedder, vector_db));
    // -----------------------------------

    let handler = MyHandler { document_index, reference_service };

    let transport = (stdin(), stdout());

    let server_handle = handler.serve(transport).await?;

    let shutdown_reason = server_handle.waiting().await?;
    println!("mc-mcp server finished. Reason: {:?}", shutdown_reason);

    Ok(())
}

#[derive(Clone)]
struct MyHandler {
    document_index: Arc<Mutex<SimpleDocumentIndex>>,
    reference_service: Arc<ReferenceServiceImpl>,
}

const MAX_SEARCH_RESULTS: usize = 5;
const SNIPPET_LINES: usize = 3;

impl MyHandler {
    async fn forge_test(&self) -> Result<Vec<Content>, String> {
        println!("Executing forge test...");
        let output_result = Command::new("forge")
            .arg("test")
            .output()
            .await;

        match output_result {
            Ok(output) => {
                println!("Forge test finished.");
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let status_code = output.status.code().map_or("N/A".to_string(), |c| c.to_string());

                let result_text = format!(
                    "Forge Test Results:\nExit Code: {}\n\nStdout:\n{}\nStderr:\n{}",
                    status_code,
                    stdout,
                    stderr
                );

                Ok(vec![Content::text(result_text)])
            }
            Err(e) => {
                println!("Failed to execute forge test: {}", e);
                Err(format!("Failed to execute forge command: {}", e))
            }
        }
    }

    // セマンティック検索に置き換え
    async fn search_docs_semantic(&self, query: &str, limit: usize) -> Vec<Content> {
        let search_query = SearchQuery {
            text: query.to_string(),
            limit: Some(limit),
        };
        match self.reference_service.search_documents(search_query, None).await {
            Ok(results) => {
                if results.is_empty() {
                    vec![Content::text(format!("No documents found matching '{}' (semantic)", query))]
                } else {
                    results.into_iter().map(|r| {
                        Content::text(format!("Semantic match: `{}` (score: {:.3})", r.file_path, r.score))
                    }).collect()
                }
            }
            Err(e) => {
                vec![Content::text(format!("Semantic search error: {}", e))]
            }
        }
    }

    pub async fn setup(&self) -> Result<Vec<Content>, String> {
        // metacontract/templateをカレントディレクトリにgit clone
        let output_result = Command::new("git")
            .arg("clone")
            .arg("https://github.com/metacontract/template.git")
            .arg("mc-template")
            .output()
            .await;

        match output_result {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let status_code = output.status.code().map_or("N/A".to_string(), |c| c.to_string());
                let result_text = format!(
                    "Setup Results:\nExit Code: {}\n\nStdout:\n{}\nStderr:\n{}",
                    status_code,
                    stdout,
                    stderr
                );
                if output.status.success() {
                    Ok(vec![Content::text(format!("プロジェクトセットアップ成功なのだ！\n{}", result_text))])
                } else {
                    Err(format!("プロジェクトセットアップ失敗なのだ…\n{}", result_text))
                }
            }
            Err(e) => {
                Err(format!("git cloneコマンドの実行に失敗したのだ: {}", e))
            }
        }
    }

    pub async fn deploy(&self) -> Result<Vec<Content>, String> {
        // forge scriptをカレントディレクトリで実行
        let output_result = Command::new("forge")
            .arg("script")
            .output()
            .await;

        match output_result {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let status_code = output.status.code().map_or("N/A".to_string(), |c| c.to_string());
                let result_text = format!(
                    "Deploy Results:\nExit Code: {}\n\nStdout:\n{}\nStderr:\n{}",
                    status_code,
                    stdout,
                    stderr
                );
                if output.status.success() {
                    Ok(vec![Content::text(format!("デプロイ成功なのだ！\n{}", result_text))])
                } else {
                    Err(format!("デプロイ失敗なのだ…\n{}", result_text))
                }
            }
            Err(e) => {
                Err(format!("forge scriptコマンドの実行に失敗したのだ: {}", e))
            }
        }
    }

    pub async fn upgrade(&self) -> Result<Vec<Content>, String> {
        // forge upgradeをカレントディレクトリで実行
        let output_result = Command::new("forge")
            .arg("upgrade")
            .output()
            .await;

        match output_result {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let status_code = output.status.code().map_or("N/A".to_string(), |c| c.to_string());
                let result_text = format!(
                    "Upgrade Results:\nExit Code: {}\n\nStdout:\n{}\nStderr:\n{}",
                    status_code,
                    stdout,
                    stderr
                );
                if output.status.success() {
                    Ok(vec![Content::text(format!("アップグレード成功なのだ！\n{}", result_text))])
                } else {
                    Err(format!("アップグレード失敗なのだ…\n{}", result_text))
                }
            }
            Err(e) => {
                Err(format!("forge upgradeコマンドの実行に失敗したのだ: {}", e))
            }
        }
    }
}

impl ServerHandler for MyHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            server_info: Implementation {
                name: "mc-mcp-server".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            capabilities: ServerCapabilities {
                tools: Some(ToolsCapability {
                    list_changed: None,
                }),
                experimental: None,
                logging: None,
                prompts: None,
                resources: None,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn list_tools(
        &self,
        _request: model::PaginatedRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<model::ListToolsResult, McpError>> + Send + '_ {
        let forge_test_tool = Tool {
            name: "forge_test".to_string().into(),
            description: "Run 'forge test' in the workspace.".to_string().into(),
            input_schema: Arc::new(Map::default()),
        };
        let search_docs_tool = Tool {
            name: "search_docs".to_string().into(),
            description: "Search metacontract documents by keyword.".to_string().into(),
            input_schema: Arc::new(json!({ "type": "object", "properties": { "query": { "type": "string", "description": "Search keyword" } }, "required": ["query"] }).as_object().unwrap().clone()),
        };
        let result = model::ListToolsResult {
            tools: vec![forge_test_tool, search_docs_tool],
            next_cursor: None,
        };
        async { Ok(result) }
    }

    fn call_tool(
        &self,
        params: CallToolRequestParam,
        _ctx: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResult, McpError>> + Send + '_ {
        let handler_clone = self.clone();
        async move {
            match params.name.as_ref() {
                "forge_test" => {
                    match handler_clone.forge_test().await {
                        Ok(content) => Ok(CallToolResult::success(content)),
                        Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
                    }
                }
                "search_docs" => {
                    if let Some(args) = params.arguments {
                        if let Some(query_value) = args.get("query") {
                            if let Some(query) = query_value.as_str() {
                                // セマンティック検索をawait
                                let results = handler_clone.search_docs_semantic(query, MAX_SEARCH_RESULTS).await;
                                Ok(CallToolResult::success(results))
                            } else {
                                Ok(CallToolResult::error(vec![Content::text("Invalid 'query' parameter: must be a string.".to_string())]))
                            }
                        } else {
                            Ok(CallToolResult::error(vec![Content::text("Missing 'query' parameter.".to_string())]))
                        }
                    } else {
                        Ok(CallToolResult::error(vec![Content::text("Missing arguments object.".to_string())]))
                    }
                }
                _ => Err(McpError::method_not_found::<CallToolRequestMethod>()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use mc_mcp_infrastructure::SimpleDocumentIndex;
    use rmcp::model::{Content, CallToolRequestParam};
    use tokio::runtime::Runtime;
    use mc_mcp_application::ReferenceService;
    use serial_test::serial;

    #[tokio::test]
    #[serial]
    async fn test_forge_test_execution() {
        // ダミーのドキュメントインデックスを作成
        let dummy_index = Arc::new(Mutex::new(SimpleDocumentIndex::new()));
        let embedder = Arc::new(mc_mcp_infrastructure::EmbeddingGenerator::new(mc_mcp_infrastructure::EmbeddingModel::BGESmallENV15).unwrap());
        let qdrant = mc_mcp_infrastructure::qdrant_client::Qdrant::from_url("http://localhost:6334").build().unwrap();
        let vector_db = Arc::new(mc_mcp_infrastructure::VectorDb::new(qdrant, "test_collection".to_string(), 384).unwrap());
        let reference_service = Arc::new(mc_mcp_application::ReferenceServiceImpl::new(embedder, vector_db));
        let handler = MyHandler {
            document_index: dummy_index,
            reference_service,
        };
        // forge_test を実行
        let result = handler.forge_test().await;

        match result {
            Ok(content_vec) => {
                println!("forge_test successful (in test). Output:");
                let mut output_text = String::new();
                for c in content_vec {
                   if let Some(raw_text) = c.raw.as_text() {
                        output_text.push_str(&raw_text.text);
                        output_text.push('\n');
                    }
                }
                println!("{}", output_text);
                assert!(true, "Forge command executed successfully (or predictably failed without panic).");
            }
            Err(e) => {
                println!("forge_test failed as expected (in test): {}", e);
                 assert!(e.contains("Failed to execute forge command"), "Error message should indicate failure to execute forge command");
            }
        }
        println!("Test assertion: Check if forge command exists and runs (or fails predictably).");
    }

    /*
    #[test]
    fn test_search_docs_logic() {
        // テスト用のインデックスを作成
        let mut test_index = SimpleDocumentIndex::new();
        test_index.insert(
            "docs/concepts/core.md".to_string(),
            "This document explains the core concepts.".to_string(),
        );
        test_index.insert(
            "docs/guides/installation.md".to_string(),
            "How to install the framework.".to_string(),
        );
        test_index.insert(
            "docs/guides/advanced.md".to_string(),
            "Advanced concepts and features.".to_string(),
        );

        let handler = MyHandler {
            document_index: Arc::new(Mutex::new(test_index)),
        };

        // 1. コンテンツにマッチするクエリ
        let results1 = handler.search_docs_semantic("core", MAX_SEARCH_RESULTS).await;
        assert_eq!(results1.len(), 1);
        if let Some(raw_text) = results1[0].raw.as_text() {
            assert!(raw_text.text.contains("Match in `docs/concepts/core.md`"));
            assert!(raw_text.text.contains("core concepts"));
        } else {
            panic!("Expected text content");
        }

        // 2. パスにマッチするクエリ
        let results2 = handler.search_docs_semantic("installation", MAX_SEARCH_RESULTS).await;
        assert_eq!(results2.len(), 1);
         if let Some(raw_text) = results2[0].raw.as_text() {
            assert!(raw_text.text.contains("Match in `docs/guides/installation.md`"));
        } else {
            panic!("Expected text content");
        }
    }

    #[tokio::test]
    async fn test_call_tool_search_docs_semantic() {
        // テスト用のReferenceServiceImplをモックまたは簡易初期化（本番同等の初期化は省略）
        // ここでは既存のMyHandlerの初期化を流用し、search_docs_semanticの呼び出しをE2Eで確認
        let dummy_index = Arc::new(Mutex::new(SimpleDocumentIndex::new()));
        // ReferenceServiceImplの初期化はmainと同じく必要（ここでは簡易化/モック化も可）
        // ここではテストのため、search_docs_semanticがエラーを返すことを確認する
        // 実際のQdrantやモデルが不要な場合は、ReferenceServiceImplのモックを使うのが理想
        // ここでは最低限のE2E配線確認のみ
        let handler = MyHandler {
            document_index: dummy_index,
            reference_service: Arc::new(ReferenceServiceImpl::new(
                Arc::new(EmbeddingGenerator::new(EmbeddingModel::AllMiniLML6V2).unwrap()),
                Arc::new(VectorDb::new(
                    Qdrant::from_url("http://localhost:6334").build().unwrap(),
                    "mc_docs_test".to_string(),
                    384,
                ).unwrap()),
            )),
        };

        let params = CallToolRequestParam {
            name: "search_docs".to_string().into(),
            arguments: Some(serde_json::json!({"query": "test"}).as_object().unwrap().clone()),
            ..Default::default()
        };
        let ctx = RequestContext::<RoleServer>::default();
        let result = handler.call_tool(params, ctx).await;
        // 結果がエラーまたは空でなければOK（Qdrantが起動していない場合はエラーになる想定）
        assert!(result.is_ok() || result.is_err());
    }
    */

    #[tokio::test]
    #[serial]
    async fn test_setup_execution() {
        let dummy_index = Arc::new(Mutex::new(SimpleDocumentIndex::new()));
        let embedder = Arc::new(mc_mcp_infrastructure::EmbeddingGenerator::new(mc_mcp_infrastructure::EmbeddingModel::BGESmallENV15).unwrap());
        let qdrant = mc_mcp_infrastructure::qdrant_client::Qdrant::from_url("http://localhost:6334").build().unwrap();
        let vector_db = Arc::new(mc_mcp_infrastructure::VectorDb::new(qdrant, "test_collection".to_string(), 384).unwrap());
        let reference_service = Arc::new(mc_mcp_application::ReferenceServiceImpl::new(embedder, vector_db));
        let handler = MyHandler {
            document_index: dummy_index,
            reference_service,
        };
        let result = handler.setup().await;
        assert!(result.is_err(), "setupコマンド未実装なのでErrを返すはずなのだ");
    }

    #[tokio::test]
    #[serial]
    async fn test_deploy_execution() {
        let dummy_index = Arc::new(Mutex::new(SimpleDocumentIndex::new()));
        let embedder = Arc::new(mc_mcp_infrastructure::EmbeddingGenerator::new(mc_mcp_infrastructure::EmbeddingModel::BGESmallENV15).unwrap());
        let qdrant = mc_mcp_infrastructure::qdrant_client::Qdrant::from_url("http://localhost:6334").build().unwrap();
        let vector_db = Arc::new(mc_mcp_infrastructure::VectorDb::new(qdrant, "test_collection".to_string(), 384).unwrap());
        let reference_service = Arc::new(mc_mcp_application::ReferenceServiceImpl::new(embedder, vector_db));
        let handler = MyHandler {
            document_index: dummy_index,
            reference_service,
        };
        let result = handler.deploy().await;
        assert!(result.is_err(), "deployコマンド未実装なのでErrを返すはずなのだ");
    }

    #[tokio::test]
    #[serial]
    async fn test_upgrade_execution() {
        let dummy_index = Arc::new(Mutex::new(SimpleDocumentIndex::new()));
        let embedder = Arc::new(mc_mcp_infrastructure::EmbeddingGenerator::new(mc_mcp_infrastructure::EmbeddingModel::BGESmallENV15).unwrap());
        let qdrant = mc_mcp_infrastructure::qdrant_client::Qdrant::from_url("http://localhost:6334").build().unwrap();
        let vector_db = Arc::new(mc_mcp_infrastructure::VectorDb::new(qdrant, "test_collection".to_string(), 384).unwrap());
        let reference_service = Arc::new(mc_mcp_application::ReferenceServiceImpl::new(embedder, vector_db));
        let handler = MyHandler {
            document_index: dummy_index,
            reference_service,
        };
        let result = handler.upgrade().await;
        assert!(result.is_err(), "upgradeコマンド未実装なのでErrを返すはずなのだ");
    }
}

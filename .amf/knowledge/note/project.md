# mc-mcp Project Plan

## 1. Project Overview

*   **Name:** mc-mcp
*   **Purpose:** Develop an MCP server for the metacontract (mc) framework, providing `tool` (Foundry integration) and `reference` (documentation search) features.
*   **Target Framework:** `metacontract` (mc) - A Foundry-based framework implementing ERC-7546 (Upgradeable Clone for Scalable Contracts) focusing on upgradeability, modularity, scalability, and testability. It is described as optimized for AI integration and DevOps efficiency .
*   **Protocol:** Model Context Protocol (MCP) - An open protocol standardizing interactions between AI applications (clients/hosts like IDEs) and external tools/data sources (servers like `mc-mcp`) .
*   **Core Functionality (MCP Tools):** Implemented using `rmcp` SDK's `#[tool]` and `#[tool(tool_box)]` annotations.
    *   **`forge_test`:** Run `forge test` in the workspace.
    *   **`search_docs`:** Offer guidance based on `mc` documentation ([https://mc-book.ecdysis.xyz](https://mc-book.ecdysis.xyz), `docs` directory in the `mc` repo) and configured sources, covering best practices, operations (including upgrades), and planning information. Utilizes internal Vector DB search capabilities. **Returns structured JSON results (`Vec<SearchResult>`).**

## 2. Technology Stack

*   **Primary Language:** **Rust**
    *   **Rationale:** Chosen over TypeScript due to superior performance (crucial for potential `reference` function complexity and Foundry interaction), strong memory safety guarantees, robust concurrency features, explicit error handling (`Result`/`Option`), excellent tooling (Cargo, rustfmt, Clippy), and strong synergy with the Rust-based Foundry ecosystem. While the learning curve is steeper, the benefits in reliability and long-term maintainability outweigh the initial development speed difference for a developer tool.
*   **Architecture:** Clean Architecture / Layered Architecture (Onion style).
*   **MCP Integration:** `modelcontextprotocol-rust-sdk`.
    *   **Design Principle:** Following the official [rmcp repository](https://github.com/modelcontextprotocol/rust-sdk) examples and community articles (e.g., [Classmethod article](https://dev.classmethod.jp/articles/mcp-rust-cursor/)), the server is started by calling `serve()` with the handler struct (implementing `ServerHandler` and `#[tool(tool_box)]`) and transport. Tool definition relies on `#[tool]` annotations on handler methods, simplifying the implementation compared to manual `list_tools`/`call_tool` overrides. **Error handling and basic server setup are stable.**
*   **Vector DB (for `reference`):** **Qdrant** (preferred) or **LanceDB**. Integrated *within* the `mc-mcp` server process.
    *   **Rationale for Internal Integration:** Aligns with MCP's local-first principle, ensures low latency (especially with `stdio`), provides self-contained functionality, simplifies `Tool` logic integration, enhances privacy, and avoids external dependencies on `mc-book` API.
*   **Text Embedding Generation (for `reference`):** Local Sentence Transformer models (e.g., `all-MiniLM-L6-v2`) using Rust libraries like `fastembed-rs` or alternatives.
*   **Configuration:** `figment`.
*   **Logging:** `tracing`.
*   **Error Handling:** Native `Result<T, E>`, `thiserror`, `anyhow`.
*   **Filesystem Operations:** `std::fs`, `tokio::fs`, `fs_extra`, `walkdir`.
*   **External Process Execution:** `tokio::process::Command`.
*   **Markdown Parsing:** `pulldown-cmark` or `comrak`.
*   **Schema Validation:** `garde` or `validator`. `schemars` used for MCP Tool argument validation via `rmcp`.
*   **Dependency Injection:** Manual DI preferred; DI containers like `shaku` if necessary.

## 3. Architecture Design (Rust)

*   **Structure:** **Library crate (`src/lib.rs`) + Binary crate (`src/main.rs`)**. Refactored from a single binary crate to improve testability and modularity.
*   **Layers (Modules within `src/`):**
    *   **`domain`:** Core business logic, entities (`mc` project, doc sections), value objects, repository traits (`trait ProjectRepository`, `trait DocumentRepository`), service traits (`trait ToolService`, `trait ReferenceService`), domain errors. No external dependencies other than core Rust/std and necessary crates like `serde`.
    *   **`application`:** Use case implementations (`struct ReferenceServiceImpl`), DTOs, application errors (`thiserror`). Depends only on the `domain` module. Injects repository traits defined in `domain`.
    *   **`infrastructure`:** Concrete repository implementations (`struct VectorDb`), Markdown parsing (`comrak`), Vector DB client (`qdrant-client` via `VectorDb` struct), embedding generation (`fastembed-rs` via `EmbeddingGenerator` struct), Foundry command execution (`tokio::process`), filesystem interaction (`tokio::fs`, `fs_extra`). Implements domain traits. Depends on the `domain` module and external crates.
    *   **`main.rs` (Binary Entry Point):** MCP protocol handling (`modelcontextprotocol-rust-sdk` with `#[tool]` annotations), transport layer (`stdio` primary), mapping MCP requests to application use cases (via annotated methods), Composition Root (manual DI), logging/config setup. Depends on the **library crate (`mc_mcp`)** which provides the application and infrastructure logic, and the MCP SDK.
*   **Dependency Rule:** Strictly inwards (Entry Point -> Library Crate (Application -> Domain <- Infrastructure)).

## 4. Core Feature Implementation Strategy

*   **`forge_test` Tool:**
    *   Implemented via `#[tool]` annotation on `MyHandler::forge_test`.
    *   Uses `tokio::process::Command` to asynchronously run `forge test`. Captures and returns stdout/stderr/exit code in the `CallToolResult`.
    *   Handles errors robustly (e.g., `forge` not found).
*   **`search_docs` Tool:**
    *   Implemented via `#[tool]` annotation on `MyHandler::search_docs_semantic`.
    *   Accepts `query` (string) and optional `limit` (integer) arguments defined in `SearchDocsArgs` struct (using `serde::Deserialize` and `schemars::JsonSchema`).
    *   **Data Sources:**
        *   **Primary:** `metacontract/mc/docs` directory (or configured path). Index for this source might be pre-built via CI/CD (TODO).
        *   **Additional:** User-configurable sources (local directories supported, Git/HTTP TODO). Specified via `mcp_config.toml`.
    *   **Parsing:** Uses `comrak` (via `infrastructure/markdown.rs`) to parse Markdown from all sources.
    *   **Indexing (Vector Search):**
        1.  **Pre-built Index (`mc` docs):** (TODO) Load the distributed index data into the local Vector DB (Qdrant).
        2.  **Additional Sources:** On startup (or via command), load configuration (`mcp_config.toml`). Iterate through configured `[reference.sources]`. For each `local` source, read documents using `walkdir`, `fs::read_to_string`.
        3.  Chunk documents meaningfully (currently simple paragraph split in `ReferenceServiceImpl::chunk_document`). Add metadata including `source` identifier, `file_path`, and the `content_chunk` itself to the Qdrant payload.
        4.  Generate text embeddings locally using `fastembed-rs` (`EmbeddingGenerator`).
        5.  Store/update chunks, embeddings, and payload (source, file_path, content_chunk, metadata) in the local Vector DB (Qdrant) via `VectorRepository::upsert_documents`.
        6.  **Search:** On query, generate query embedding, perform similarity search in the Vector DB via `VectorRepository::search`. Optionally filter by source metadata (TODO).
        7.  **Return Results:** Returns structured JSON (`Vec<SearchResult>`) via `Content::json()` in the `CallToolResult`.
    *   **Status:**
        *   Semantic search pipeline implemented and tests stabilized.
        *   **Configuration loading (`figment`, `mcp_config.toml`) for sources implemented.** [configuration spec](./config.md)
        *   **Indexing logic updated to handle multiple configured local sources.**
        *   **Vector DB interaction updated to store and retrieve source, content_chunk, and metadata in payload.**
        *   **MCP Tool implemented using `#[tool]` annotation.**
        *   **MCP Tool response updated to return structured JSON.** (Completed)
    *   **Next:** Implement pre-built index loading. Support Git/HTTP sources. Add search filtering.

## 5. Supporting Infrastructure

*   **Configuration:** Use `figment` for flexible loading from files (e.g., TOML) and environment variables. Define config structures (`serde`) including sections for specifying `reference` data sources (type, location/URL, path within repo). Use `serde` for defining config structures.
*   **Logging:** Use `tracing` with `tracing-subscriber` for structured, asynchronous logging. Log to stderr for `stdio` transport.
*   **Error Handling:** Use `Result<T, E>` extensively. Define custom error types per layer using `thiserror`. Use `anyhow` for application-level errors where appropriate. Propagate errors clearly. Avoid `panic!`.
*   **Data Persistence:** Use the local file system for caching. Use the chosen Vector DB (Qdrant via `qdrant-client`) for `reference` function indexes. Consider SQLite (`rusqlite`) if a non-vector structured index is needed.
*   **MCP Transport:** Implement **`stdio`** as the primary transport mechanism . Consider adding `SSE` support later only if a clear remote/shared use case emerges, acknowledging the added complexity and security implications .
*   **Testing:** Employ Test-Driven Development (TDD) principles. Use standard Rust unit tests (`#[test]`). For infrastructure components interacting with external systems (like Qdrant), use integration tests (`#[cfg(test)]`) leveraging `testcontainers` to run services (e.g., Qdrant) in Docker during test execution. Use `serial_test` to manage tests relying on shared resources like containers or environment variables. **Integration tests using `testcontainers` have been stabilized and now pass reliably after corrections: Qdrant startup wait condition changed to stdout log, gRPC port exposure, test data assertion adjustment. TDD usage and CI/CD also have sufficient stability confirmed.**

## 6. Project Roadmap (Phased Approach)

*   **Phase 1: Foundation & Core `tool` (Rust)**
    *   Setup Cargo workspace, layered crates. (Completed - Evolved to Lib+Bin)
    *   Basic MCP server skeleton (`stdio` transport, `modelcontextprotocol-rust-sdk`). (Completed)
    *   Implement core `tool` logic (e.g., `forge test` execution via `tokio::process`). (Completed)
    *   Setup basic logging, error handling, configuration. (Completed)
    *   Initial unit/integration tests. **(Completed - Build/Unit tests stabilized, Integration tests under investigation)**
    *   **Goal:** Runnable MCP server executing `forge test`. (Achieved)
*   **Phase 2: Basic `reference` Function**
    *   Implement Markdown parsing (`comrak`). (Completed)
    *   Implement basic keyword/structure-based indexing (Superseded by Semantic Search). (Completed)
    *   Implement MCP `Tool` for basic document search. (Completed)
    *   Refine Application/Domain layers for `reference`. (Completed)
    *   Add tests for parsing/searching. (Completed)
    *   **Goal:** Search `mc` docs via keyword/structure. (Achieved)
*   **Phase 3: Advanced `reference` (Semantic Search - Recommended)**
    *   Integrate local embedding model (`fastembed-rs` - `EmbeddingGenerator` implemented). (Completed)
    *   Setup local Vector DB (Qdrant - `qdrant-client` and `VectorDb` struct implemented). (Completed)
    *   Implement embedding generation pipeline for parsed docs (Completed - Pipeline logic exists). (Completed)
    *   Implement similarity search logic (Completed - Integrated `VectorDb::search` in Application layer). (Completed)
    *   **Implement configuration handling (`figment`) and indexing logic for additional user-defined local document sources.** (Completed)
    *   **Update VectorDB interaction to handle source metadata, content chunks in payload.** (Completed)
    *   **Refactor MCP Tool implementation to use `#[tool]` annotations.** (Completed)
    *   **Update MCP `search_docs` tool to return structured JSON results.** (Completed)
    *   **(Next)** Implement logic to load pre-built `mc` docs index.
    *   **(TODO)** Implement search filtering by source.
    *   **Integration tests for the full ReferenceService pipeline passing. (Completed - Build/Unit tests and Integration tests both stabilized and confirmed full pass)**
    *   **Goal:** Semantic search over `mc` docs *and* user-added local docs via natural language query, using modern `rmcp` practices and returning structured results. (Achieved)
*   **Phase 4: `tool` Expansion & Polish**
    *   **(TODO)** Implement remaining `tool` functions (setup, deploy, upgrade).
    *   Improve error handling and user feedback via MCP.
    *   Increase test coverage.
    *   Develop documentation.
    *   **Goal:** Comprehensive `mc` development workflow support.

## 7. Next Steps

1.  Confirm final acceptance of Rust and internal Vector DB approach. (Completed)
2.  Set up the initial Cargo workspace structure. (Completed)
3.  Implement Phase 1. (Completed)
4.  Implement Phase 2 (Superseded by Phase 3). (Completed)
5.  Implement Phase 3:
    *   `VectorDb` and `EmbeddingGenerator` implemented and tested. (Completed)
    *   Application layer integration completed. (Completed)
    *   Integration tests for the ReferenceService pipeline passing. **(Completed - Build/Unit tests stabilized, Integration tests under investigation)**
    *   Configuration loading and indexing for multiple local sources implemented. (Completed)
    *   Vector DB payload updated for source, chunk, metadata. (Completed)
    *   MCP Tool implementation refactored to use `#[tool]` annotations. (Completed)
    *   **MCP `search_docs` tool updated to return structured JSON results.** (Completed)
    *   **(Completed)** Refactor project to Library + Binary structure.
    *   **(Completed)** Fix build errors and stabilize unit tests after refactoring.
    *   **(Next)** Implement pre-built index loading.
    *   **(TODO)** Implement search filtering by source.
6.  Establish CI/CD pipeline (including pre-building `mc` docs index). (TODO)
7.  Monitor MCP specification and `metacontract` evolution for necessary adaptations. (Ongoing)
8.  Implement Phase 4 (Tool expansion, polish). (TODO)

## 8. Recommended TDD Practices

*   Integration tests involving Qdrant or other external dependencies are now stable and pass reliably by ensuring TempDir lifecycle management, explicit score_threshold usage, serializing Docker resource usage, and thoroughly correct Qdrant startup wait condition (stdout log) and gRPC port exposure and test data adjustment.
*   When a test fails, troubleshoot in the order: external service startup → test logic → threshold design.
*   The project is now well-suited for a robust TDD cycle (Red → Green → Refactor) with maintainable test and implementation structure.

## 9. TDD Coverage Reporting

- Unless otherwise specified, coverage reports should be output in human-readable text format using `cargo llvm-cov --report-type=summary`.
- Markdown or HTML formats may be used in addition for CI or review purposes if needed.

## 10. Test Result Interpretation by Cursor (AI Assistant)

- Cursor (AI assistant) must always check the final summary line of `cargo test` output (e.g., `test result: ok. N passed; 0 failed; ...`) before declaring "all tests passed".
- If the test run is interrupted (e.g., by Ctrl+C) or if there are any failed tests, Cursor must **not** claim that all tests passed.
- If there is any ambiguity (e.g., partial output, missing summary, or signs of cancellation), Cursor should report the situation and ask the user for clarification, rather than assuming success.
- This is especially important for long-running or integration tests (e.g., Qdrant, Docker-based tests) where partial output may be misleading.

## Milestones for Reference Source Types

- [Completed] Support for relative paths to local files/directories in the repository.
- [Next] Support for pre-built index loading (likely from a specific configured path).
- [Future] Support for Markdown files published on GitHub.
- [Future] Support for arbitrary URL content.

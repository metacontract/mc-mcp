# mc-mcp Project Plan

## 1. Project Overview

*   **Name:** mc-mcp
*   **Purpose:** Develop an MCP server for the metacontract (mc) framework, providing `tool` (Foundry integration) and `reference` (documentation search) features.
*   **Target Framework:** `metacontract` (mc) - A Foundry-based framework implementing ERC-7546 (Upgradeable Clone for Scalable Contracts) focusing on upgradeability, modularity, scalability, and testability. It is described as optimized for AI integration and DevOps efficiency .
*   **Protocol:** Model Context Protocol (MCP) - An open protocol standardizing interactions between AI applications (clients/hosts like IDEs) and external tools/data sources (servers like `mc-mcp`) .
*   **Core Functionality (MCP Tools):**
    *   **`tool`:** Provide capabilities for `mc` project setup, running test/deploy/upgrade scripts (interacting with Foundry commands like `forge test` [1]), and managing project state. This will be implemented as one or more MCP `Tools`.
    *   **`reference`:** Offer guidance based on `mc` documentation ([https://mc-book.ecdysis.xyz](https://mc-book.ecdysis.xyz), `docs` directory in the `mc` repo) covering best practices, operations (including upgrades), and planning information. This will also be implemented as an MCP `Tool`, utilizing internal Vector DB search capabilities.

## 2. Technology Stack

*   **Primary Language:** **Rust**
    *   **Rationale:** Chosen over TypeScript due to superior performance (crucial for potential `reference` function complexity and Foundry interaction), strong memory safety guarantees, robust concurrency features, explicit error handling (`Result`/`Option`), excellent tooling (Cargo, rustfmt, Clippy), and strong synergy with the Rust-based Foundry ecosystem. While the learning curve is steeper, the benefits in reliability and long-term maintainability outweigh the initial development speed difference for a developer tool.
*   **Architecture:** Clean Architecture / Layered Architecture (Onion style).
*   **MCP Integration:** `modelcontextprotocol-rust-sdk`.
    *   **Design Principle:** Following the official [rmcp repository](https://github.com/modelcontextprotocol/rust-sdk), there is no need to manage or initialize Peer types manually. The server should be started simply by calling `serve()` with the service and transport. The `peer` field in `RequestContext` can be omitted in tests and is not required for normal operation. **All related type errors are now resolved!**
*   **Vector DB (for `reference`):** **Qdrant** (preferred) or **LanceDB**. Integrated *within* the `mc-mcp` server process.
    *   **Rationale for Internal Integration:** Aligns with MCP's local-first principle, ensures low latency (especially with `stdio`), provides self-contained functionality, simplifies `Tool` logic integration, enhances privacy, and avoids external dependencies on `mc-book` API.
*   **Text Embedding Generation (for `reference`):** Local Sentence Transformer models (e.g., `all-MiniLM-L6-v2`) using Rust libraries like `fastembed-rs` or alternatives.
*   **Configuration:** `figment`.
*   **Logging:** `tracing`.
*   **Error Handling:** Native `Result<T, E>`, `thiserror`.
*   **Filesystem Operations:** `std::fs`, `tokio::fs`, `fs_extra`, `walkdir`.
*   **External Process Execution:** `tokio::process::Command`.
*   **Markdown Parsing:** `pulldown-cmark` or `comrak`.
*   **Schema Validation:** `garde` or `validator`.
*   **Dependency Injection:** Manual DI preferred; DI containers like `shaku` if necessary.

## 3. Architecture Design (Rust)

*   **Structure:** Single crate (`mc-mcp`) with layered modules (`src/domain`, `src/application`, `src/infrastructure`).
*   **Layers (Modules within `src/`):**
    *   **`domain`:** Core business logic, entities (`mc` project, doc sections), value objects, repository traits (`trait ProjectRepository`, `trait DocumentRepository`), service traits (`trait ToolService`, `trait ReferenceService`), domain errors. No external dependencies other than core Rust/std and necessary crates like `serde`.
    *   **`application`:** Use case implementations (`struct ReferenceServiceImpl`), DTOs, application errors (`thiserror`). Depends only on the `domain` module. Injects repository traits defined in `domain`.
    *   **`infrastructure`:** Concrete repository implementations (`struct VectorDb`), Markdown parsing (`comrak`), Vector DB client (`qdrant-client` via `VectorDb` struct), embedding generation (`fastembed-rs` via `EmbeddingGenerator` struct), Foundry command execution (`tokio::process`), filesystem interaction (`tokio::fs`, `fs_extra`). Implements domain traits. Depends on the `domain` module and external crates.
    *   **`main.rs` (Binary Entry Point):** MCP protocol handling (`modelcontextprotocol-rust-sdk`), transport layer (`stdio` primary), mapping MCP requests to application use cases, Composition Root (manual DI), logging/config setup. Depends on the `application` and `infrastructure` modules, and the MCP SDK.
*   **Dependency Rule:** Strictly inwards (Entry Point -> Application -> Domain <- Infrastructure).

## 4. Core Feature Implementation Strategy

*   **`tool` Function:**
    *   **Project Setup:** Use Rust FS libraries (`fs_extra`) to manipulate files based on `metacontract/template` . Execute `git` commands via `tokio::process`.
    *   **Script Execution:** Use `tokio::process::Command` to asynchronously run Foundry commands (`forge test`, `forge script`, etc.). Capture and parse stdout/stderr/exit codes. Handle errors robustly. Ensure non-blocking execution.
    *   **Security:** Validate inputs rigorously. Require explicit user confirmation via the MCP client for any command execution.
*   **`reference` Function:**
    *   **Data Sources:**
        *   **Primary:** `metacontract/mc/docs` directory (or configured path). Index for this source might be pre-built via CI/CD (TODO).
        *   **Additional:** User-configurable sources (local directories supported, Git/HTTP TODO). Specified via `mcp_config.toml`.
    *   **Parsing:** Use `pulldown-cmark` or `comrak` to parse Markdown from all sources.
    *   **Indexing (Vector Search):**
        1.  **Pre-built Index (`mc` docs):** (TODO) Load the distributed index data into the local Vector DB (Qdrant).
        2.  **Additional Sources:** On startup (or via command), load configuration (`mcp_config.toml`). Iterate through configured `[reference.sources]`. For each `local` source, read documents using `walkdir`, `fs::read_to_string`.
        3.  Chunk documents meaningfully (currently simple paragraph split in `ReferenceServiceImpl::chunk_document`). Add metadata including `source` identifier, `file_path`, and the `content_chunk` itself to the Qdrant payload.
        4.  Generate text embeddings locally using `fastembed-rs` (`EmbeddingGenerator`).
        5.  Store/update chunks, embeddings, and payload (source, file_path, content_chunk, metadata) in the local Vector DB (Qdrant) via `VectorRepository::upsert_documents`.
        6.  **Search:** On query, generate query embedding, perform similarity search in the Vector DB via `VectorRepository::search`. Optionally filter by source metadata (TODO).
        7.  Return relevant results (`Vec<SearchResult>`) including file path, score, source identifier, content chunk, and metadata via MCP `Tool` response.
    *   **MCP Implementation:** Expose search functionality as an MCP `Tool` accepting natural language queries. **(Needs update to return richer `SearchResult` data)**.
    *   **Status:**
        *   Semantic search pipeline implemented and tests stabilized.
        *   **Configuration loading (`figment`, `mcp_config.toml`) for sources implemented.**
        *   **Indexing logic updated to handle multiple configured local sources.**
        *   **Vector DB interaction updated to store and retrieve source, content_chunk, and metadata in payload.**
    *   **Next:** Implement pre-built index loading. Update MCP Tool response. Support Git/HTTP sources. Add search filtering.

## 5. Supporting Infrastructure

*   **Configuration:** Use `figment` for flexible loading from files (e.g., TOML) and environment variables. Define config structures (`serde`) including sections for specifying `reference` data sources (type, location/URL, path within repo). Use `serde` for defining config structures.
*   **Logging:** Use `tracing` with `tracing-subscriber` for structured, asynchronous logging. Log to stderr for `stdio` transport.
*   **Error Handling:** Use `Result<T, E>` extensively. Define custom error types per layer using `thiserror`. Propagate errors clearly. Avoid `panic!`.
*   **Data Persistence:** Use the local file system for caching. Use the chosen Vector DB (Qdrant via `qdrant-client`) for `reference` function indexes. Consider SQLite (`rusqlite`) if a non-vector structured index is needed.
*   **MCP Transport:** Implement **`stdio`** as the primary transport mechanism . Consider adding `SSE` support later only if a clear remote/shared use case emerges, acknowledging the added complexity and security implications .
*   **Testing:** Employ Test-Driven Development (TDD) principles. Use standard Rust unit tests (`#[test]`). For infrastructure components interacting with external systems (like Qdrant), use integration tests (`#[cfg(test)]`) leveraging `testcontainers` to run services (e.g., Qdrant) in Docker during test execution. Use `serial_test` to manage tests relying on shared resources like containers or environment variables.

## 6. Project Roadmap (Phased Approach)

*   **Phase 1: Foundation & Core `tool` (Rust)**
    *   Setup Cargo workspace, layered crates.
    *   Basic MCP server skeleton (`stdio` transport, `modelcontextprotocol-rust-sdk`).
    *   Implement core `tool` logic (e.g., `forge test` execution via `tokio::process`).
    *   Setup basic logging, error handling, configuration.
    *   Initial unit/integration tests.
    *   **Goal:** Runnable MCP server executing `forge test`. (Completed)
*   **Phase 2: Basic `reference` Function**
    *   Implement Markdown parsing (`pulldown-cmark`/`comrak`).
    *   Implement basic keyword/structure-based indexing (non-vector).
    *   Implement MCP `Tool` for basic document search.
    *   Refine Application/Domain layers for `reference`.
    *   Add tests for parsing/searching.
    *   **Goal:** Search `mc` docs via keyword/structure. (Completed)
*   **Phase 3: Advanced `reference` (Semantic Search - Recommended)**
    *   Integrate local embedding model (`fastembed-rs` - `EmbeddingGenerator` implemented).
    *   Setup local Vector DB (Qdrant - `qdrant-client` and `VectorDb` struct implemented).
    *   Implement embedding generation pipeline for parsed docs (Completed - Pipeline logic exists).
    *   Implement similarity search logic (Completed - Integrated `VectorDb::search` in Application layer).
    *   **Implement configuration handling (`figment`) and indexing logic for additional user-defined local document sources.** (Completed)
    *   **Update VectorDB interaction to handle source metadata, content chunks in payload.** (Completed)
    *   **(TODO)** Implement logic to load pre-built `mc` docs index.
    *   **(TODO)** Update MCP `Tool` for semantic search to return richer `SearchResult` info (source, chunk, metadata).
    *   Add integration tests for the full ReferenceService pipeline (Completed - Core tests passing).
    *   **Goal:** Semantic search over `mc` docs *and* user-added local docs via natural language query.
*   **Phase 4: `tool` Expansion & Polish**
    *   Implement remaining `tool` functions (setup, deploy, upgrade).
    *   Improve error handling and user feedback via MCP.
    *   Increase test coverage.
    *   Develop documentation.
    *   **Goal:** Comprehensive `mc` development workflow support.

## 7. Next Steps

1.  Confirm final acceptance of Rust and internal Vector DB approach. (Completed)
2.  Set up the initial Cargo workspace structure. (Completed)
3.  Implement Phase 1. (Completed)
4.  Implement Phase 4. (Completed)
5.  Implement Phase 3:
    *   `VectorDb` and `EmbeddingGenerator` implemented and tested. (Completed)
    *   Application layer integration completed. (Completed)
    *   Integration tests for the ReferenceService pipeline passing. (Completed)
    *   **Configuration loading and indexing for multiple local sources implemented.** (Completed)
    *   **Vector DB payload updated for source, chunk, metadata.** (Completed)
    *   **(TODO)** Implement pre-built index loading.
    *   **(TODO)** Update the MCP `Tool` (`search_docs` in `main.rs`) to utilize the enhanced semantic search functionality and return richer results.
6.  Establish CI/CD pipeline (including pre-building `mc` docs index). (TODO)
7.  Monitor MCP specification and `metacontract` evolution for necessary adaptations. (Ongoing)
8.  [Completed] Fix test code after single-crate migration.
9.  [Completed] Prebuilt mc-docs index loading (TDD cycle).
10. **[Completed]** Additional document source config/indexing (local paths), metadata integration (source, chunk).
11. **[Next]** Update MCP `search_docs` tool in `main.rs` to return richer results.
12. **[Next]** Implement pre-built index loading.
13. **[Next]** Implement search filtering by source.
14. **[Future]** Support Git/HTTP sources.

## 8. Recommended TDD Practices

*   Integration tests involving Qdrant or other external dependencies are now stable by ensuring TempDir lifecycle management, explicit score_threshold usage, and serializing Docker resource usage.
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
- [Milestone 1] Support for Markdown files published on GitHub.
- [Milestone 2] Support for arbitrary URL content.

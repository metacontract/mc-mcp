# mc-mcp Project Plan

## 1. Project Overview

*   **Name:** mc-mcp
*   **Purpose:** Develop a Model Context Protocol (MCP) server to support smart contract development using the `metacontract` (mc) framework ([https://github.com/metacontract/mc](https://github.com/metacontract/mc)). The server will provide tools and reference information to enhance the developer experience.
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

*   **Structure:** Cargo workspace with layered crates.
*   **Layers:**
    *   **`mc-mcp-domain`:** Core business logic, entities (`mc` project, doc sections), value objects, repository traits (`trait ProjectRepository`, `trait DocumentRepository`), service traits (`trait ToolService`, `trait ReferenceService`), domain errors. No external dependencies other than core Rust/std.
    *   **`mc-mcp-application`:** Use case implementations (`struct ToolServiceImpl`), DTOs, application errors (`thiserror`). Depends only on `mc-mcp-domain`. Injects repository traits.
    *   **`mc-mcp-infrastructure`:** Concrete repository implementations (`struct FileSystemProjectRepository`, `struct VectorDBDocumentRepository`), Markdown parsing (`comrak`), Vector DB client (`qdrant-client` via `VectorDb` struct with `new`, `initialize_collection`, `upsert_documents`, `search` methods), embedding generation (`fastembed-rs` via `EmbeddingGenerator` struct), Foundry command execution (`tokio::process`), filesystem interaction (`tokio::fs`, `fs_extra`). Implements domain traits. Depends on `mc-mcp-domain` and external crates. Includes integration tests using `testcontainers` for Qdrant.
    *   **`mc-mcp-server` (Binary Crate):** MCP protocol handling (`modelcontextprotocol-rust-sdk`), transport layer (`stdio` primary), mapping MCP requests to application use cases, Composition Root (manual DI), logging/config setup. Depends on `mc-mcp-application` and MCP SDK.
*   **Dependency Rule:** Strictly inwards (Presentation -> Application -> Domain <- Infrastructure).

## 4. Core Feature Implementation Strategy

*   **`tool` Function:**
    *   **Project Setup:** Use Rust FS libraries (`fs_extra`) to manipulate files based on `metacontract/template` . Execute `git` commands via `tokio::process`.
    *   **Script Execution:** Use `tokio::process::Command` to asynchronously run Foundry commands (`forge test`, `forge script`, etc.). Capture and parse stdout/stderr/exit codes. Handle errors robustly. Ensure non-blocking execution.
    *   **Security:** Validate inputs rigorously. Require explicit user confirmation via the MCP client for any command execution.
*   **`reference` Function:**
    *   **Data Source:** Markdown files in `metacontract/mc/docs`.
    *   **Parsing:** Use `pulldown-cmark` or `comrak` to parse Markdown.
    *   **Indexing (Vector Search):**
        1.  Chunk documents meaningfully.
        2.  Generate text embeddings locally using `fastembed-rs` or similar Rust library with a model like `all-MiniLM-L6-v2`.
        3.  Store chunks and embeddings in a local Vector DB (Qdrant via Docker or LanceDB embedded).
        4.  On query: generate query embedding, perform similarity search in Vector DB.
        5.  Return relevant chunks via MCP `Tool` response.
    *   **MCP Implementation:** Expose search functionality as an MCP `Tool` accepting natural language queries.

## 5. Supporting Infrastructure

*   **Configuration:** Use `figment` for flexible loading from files (e.g., TOML) and environment variables. Use `serde` for defining config structures.
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
    *   **Goal:** Runnable MCP server executing `forge test`.
*   **Phase 2: Basic `reference` Function**
    *   Implement Markdown parsing (`pulldown-cmark`/`comrak`).
    *   Implement basic keyword/structure-based indexing (non-vector).
    *   Implement MCP `Tool` for basic document search.
    *   Refine Application/Domain layers for `reference`.
    *   Add tests for parsing/searching.
    *   **Goal:** Search `mc` docs via keyword/structure.
*   **Phase 3: Advanced `reference` (Semantic Search - Recommended)**
    *   Integrate local embedding model (`fastembed-rs` - `EmbeddingGenerator` implemented).
    *   Setup local Vector DB (Qdrant - `qdrant-client` and `VectorDb` struct implemented).
    *   Implement embedding generation pipeline for parsed docs (In Progress - Handling Errors).
    *   Implement similarity search logic (In Progress - Handling Errors - Use `VectorDb::search` in Application layer).
    *   Update MCP `Tool` for semantic search (TODO).
    *   Add tests for embedding/semantic search (`VectorDb` integration tests using `testcontainers` implemented; pipeline tests needed).
    *   **Goal:** Semantic search over `mc` docs via natural language query.
*   **Phase 4: `tool` Expansion & Polish**
    *   Implement remaining `tool` functions (setup, deploy, upgrade).
    *   Improve error handling and user feedback via MCP.
    *   Increase test coverage.
    *   Develop documentation.
    *   **Goal:** Comprehensive `mc` development workflow support.

## 7. Next Steps

1.  Confirm final acceptance of Rust and internal Vector DB approach.
2.  Set up the initial Cargo workspace structure.
3.  Begin implementation of Phase 1, focusing on the MCP server skeleton and `forge test` execution. -> Phase 2 completed -> Phase 3 in progress (`VectorDb` implemented and tested, experiencing errors in Application layer integration).
4.  Establish CI/CD pipeline.
5.  Monitor MCP specification and `metacontract`  evolution for necessary adaptations.

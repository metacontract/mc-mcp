# mc-mcp Project Plan (Updated)

## 1. Project Overview

*   **Name:** mc-mcp
*   **Purpose:** Develop an MCP server for the metacontract (mc) framework, providing `tool` (Foundry integration) and `reference` (documentation search) features.
*   **Target Framework:** `metacontract` (mc) - A Foundry-based framework implementing ERC-7546 (Upgradeable Clone for Scalable Contracts) focusing on upgradeability, modularity, scalability, and testability. Optimized for AI integration and DevOps efficiency.
*   **Protocol:** Model Context Protocol (MCP) - An open protocol standardizing interactions between AI applications (clients/hosts like IDEs) and external tools/data sources (servers like `mc-mcp`).
*   **Core Functionality (MCP Tools):** Implemented using `rmcp` SDK's `#[tool]` and `#[tool(tool_box)]` annotations.
    *   **Naming convention:** All MCP tools should be named with the `mc_` prefix followed by the function name (e.g., `mc_setup`, `mc_deploy`, `mc_upgrade`, `mc_lint`, `mc_test`, `mc_search_docs`).
    *   **Deploy/Upgrade tool dry-run support:** `mc_deploy` and `mc_upgrade` must support dry-run mode (default), toggled by the `broadcast: true` argument. The script path is configured via `mcp_config.toml ([scripts].deploy / [scripts].upgrade)`. Without `broadcast: true`, the tool should simulate the operation (dry-run); with `broadcast: true`, it should execute the actual deployment/upgrade (requires `rpc_url` and `private_key_env_var` to be set in `mcp_config.toml` under `[scripts]`).
    *   **`mc_test`:** Run `forge test` in the workspace.
    *   **`mc_search_docs`:** Semantic search over `mc` documentation and user-configured sources, returning structured JSON (`Vec<SearchResult>`).
    *   **`mc_setup`**: Initialize a new Foundry project using the `metacontract/template`. Requires an empty directory.

## 2. Technology Stack

*   **Primary Language:** Rust
*   **Architecture:** Layered (Onion) Architecture
*   **MCP Integration:** `modelcontextprotocol-rust-sdk` with `#[tool]` annotations
*   **Vector DB:** Qdrant (embedded, managed via `qdrant-client`)
*   **Text Embedding:** Local Sentence Transformer (e.g., all-MiniLM-L6-v2) via `fastembed-rs`
*   **Configuration:** figment
*   **Logging:** tracing
*   **Error Handling:** Result<T, E>, thiserror, anyhow
*   **Markdown Parsing:** comrak
*   **Dependency Injection:** Manual DI

## 3. Architecture Design

*   **Structure:** Library crate (`src/lib.rs`) + Binary crate (`src/main.rs`)
*   **Layers:**
    *   `domain`: Core logic, traits, entities
    *   `application`: Use cases, DTOs, depends on `domain`
    *   `infrastructure`: Integrations (Qdrant, embeddings, file system, etc.), depends on `domain`
    *   `main.rs`: Entry point, DI, MCP tool registration
*   **Dependency Rule:** Strictly inward (Entry Point -> Library Crate (Application -> Domain <- Infrastructure))

## 4. Project Progress & Task Management

**For up-to-date milestones, progress, and detailed task tracking, see:**
- [mc-mcp Task List](../task/task-list.md)

> Implementation progress, TODOs, and milestone status are managed in `task-list.md`. This document focuses on overall design, architecture, and technical choices.

## 5. Supporting Infrastructure

*   **Configuration:** figment, TOML config, env vars
*   **Logging:** tracing, logs to stderr for stdio
*   **Error Handling:** Result<T, E>, thiserror, anyhow
*   **Persistence:** Local file system for cache, Qdrant for vector index
*   **Transport:** stdio (primary), SSE (future)
*   **Testing:** TDD, unit/integration tests. Qdrant integration tests utilize `testcontainers` for isolated and stable execution. Unit tests involving `forge` use mock scripts (`tests/mock_bin/forge`).
*   **CI/CD:** GitHub Actions (build, test, prebuilt index build & upload)
    *   Artifacts: prebuilt_index.jsonl.gz (compressed, not in git)
    *   Download logic: always fetches latest from GitHub Releases if not present

## 6. Project Root & Configuration

- **The project root must always be specified via the `MC_PROJECT_ROOT` environment variable.**
    - All commands and tools use this directory as the base for all operations.
    - The configuration file (`mcp_config.toml`) must be placed directly under this directory.
    - If `MC_PROJECT_ROOT` is not set or does not exist, mc-mcp will return an error and not start.
    - If `mcp_config.toml` is missing, mc-mcp will start with built-in defaults and log a warning.

### Example: Startup

```sh
export MC_PROJECT_ROOT=/path/to/your/project
mc-mcp
```

### Example: Directory Structure

```
/path/to/your/project/
├── mcp_config.toml   # (optional)
├── contracts/
├── scripts/
└── ...
```

## 7. Usage & Distribution

*   **crates.io:** Project is ready for crates.io publication (see mcp_config.toml)
*   **Prebuilt index:** Not included in crate; always downloaded from GitHub Releases (latest)
*   **.gitignore:** Artifacts and large files are excluded from git
*   **README:** To be generated from this document

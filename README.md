# mc-mcp

A Model Context Protocol (MCP) server for the [metacontract (mc)](https://github.com/metacontract/mc) framework, providing smart contract development support via Foundry integration and semantic documentation search.

---

## Overview

**mc-mcp** is an extensible MCP server designed for the [metacontract](https://github.com/metacontract/mc) (mc) framework.
It enables AI-powered smart contract development workflows by exposing tools such as:

- **`mc_search_docs`**: Perform semantic search over mc documentation and user-configured sources, returning structured JSON results.
- **`mc_test`**: Run Foundry tests (`forge test`) in your workspace.
- **`mc_search_docs`**: Semantic search over mc documentation and user-configured sources, returning structured JSON results.
- **`mc_deploy`**: Deploy contracts (with `--broadcast`: production execution, without: dry-run simulation).
- **`mc_upgrade`**: Upgrade contracts (with `--broadcast`: production execution, without: dry-run simulation).
- **`mc_init`**: Initialize a new mc project or environment.
- **`mc_lint`**: Lint project files for best practices and errors.

---

## Features

- **Layered (Onion) Architecture** for maintainability and testability
- **Rust** implementation for performance and safety
- **MCP SDK** (`modelcontextprotocol-rust-sdk`) with `#[tool]` annotation-based tool definition
- **Qdrant** (embedded vector DB) for fast semantic search
- **Local embedding generation** using [fastembed-rs](https://github.com/kuvaus/fastembed-rs)
- **Markdown parsing** with [comrak](https://github.com/kivikakk/comrak)
- **Flexible configuration** via [figment](https://github.com/SergioBenitez/figment)
- **Structured logging** with [tracing](https://github.com/tokio-rs/tracing)
- **CI/CD**: Prebuilt documentation index is generated, compressed, and published as a GitHub Release asset

---

## Architecture

- **Library + Binary crate** structure
- **Domain / Application / Infrastructure** layering
- **Manual Dependency Injection**
- **Strict inward dependency rule** (Entry Point → Application → Domain ← Infrastructure)
- **Prebuilt index**: Downloaded automatically from GitHub Releases on first run (not included in the crate)

---

## Getting Started

### 1. Install mc-mcp (as a CLI tool)

# If published on crates.io (recommended for most users)
cargo install mc-mcp

# Or, to use the latest development version from GitHub:
cargo install --git https://github.com/metacontract/mc-mcp.git

# (Alternatively, you can clone and build manually:)
git clone https://github.com/metacontract/mc-mcp.git
cd mc-mcp
cargo build --release

### 2. Initialize a new mc project

```sh
forge init <your-project-name> -t metacontract/template
cd <your-project-name>
```

### 3. Start the mc-mcp server

```sh
mc-mcp
# or
/path/to/mc-mcp/target/release/mc-mcp
```

- On first startup, the prebuilt documentation index (`prebuilt_index.jsonl.gz`) will be downloaded from the latest GitHub Release if not present.

### 4. Connect with an MCP client

mc-mcp works with any MCP-compatible client, such as:
- [Cursor](https://cursor.so/)
- [Cline](https://github.com/cline/cline)
- [RooCode](https://github.com/RooVetGit/Roo-Code)

#### Example: Cursor

Add your MCP server in the settings (`mcp.json`):

```json
{
  "mcpServers": {
    "mc-mcp": {
      "command": "mc-mcp", // or "/path/to/mc-mcp/target/release/mc-mcp",
      "args": [],
      "cwd": "/path/to/your/project"
    }
  }
}
```
#### Example: Cline

Cline auto-detects MCP servers in your workspace.
For advanced configuration, see [Cline's documentation](https://github.com/cline/cline).

#### Example: RooCode

RooCode supports MCP integration out of the box.
See [RooCode's documentation](https://github.com/RooVetGit/Roo-Code) for details.

### 5. Develop your smart contract

- Use your MCP-compatible Agent/IDE to interact with mc-mcp
- Design, search docs, and run TDD cycles (`mc_test`, `mc_search_docs`, etc.)

### Configuration Example

Create a file named `mcp_config.toml` in your project root:

```toml
[reference]
prebuilt_index_path = "artifacts/prebuilt_index.jsonl.gz"

[[reference.sources]]
name = "mc-docs"
source_type = "local"
path = "metacontract/mc/site/docs"

[[reference.sources]]
name = "solidity-docs"
source_type = "local"
path = "docs/solidity"

[[reference.sources]]
name = "user-docs"
source_type = "local"
path = "docs/user"
```

- `prebuilt_index_path` ... (optional) Path to a prebuilt index (jsonl or gzipped jsonl). If set, it will be loaded and upserted into Qdrant on startup.
- Each `[[reference.sources]]` must have `name`, `source_type` (usually `local`), and `path` (relative to the execution directory).
- All paths must exist and be directories, or indexing will fail.

See also: [config.rs](src/config.rs) for the full config structure.

---

## Project Structure

- `src/domain/` — Core business logic, traits, entities
- `src/application/` — Use cases, DTOs
- `src/infrastructure/` — Integrations (Qdrant, embeddings, file system, etc.)
- `src/main.rs` — Entry point, MCP tool registration, DI

---

## Development

- **TDD**: Test-Driven Development is enforced (unit/integration tests)
- **Integration tests**: Use [testcontainers](https://github.com/testcontainers/testcontainers-rs) for Qdrant
- **CI/CD**: GitHub Actions for build, test, and prebuilt index artifact management

## Testing & Troubleshooting

Some tests (especially integration tests and embedding-related tests) may fail due to OS file descriptor limits or cache lock issues.

### Common Commands (via Makefile)

- **Unit tests (lib only, with cache lock cleanup, single-threaded):**
  ```sh
  make test-lib
  ```
- **All tests (recommended: increase ulimit before running):**
  ```sh
  ulimit -n 4096
  make test-all
  ```
- **Integration tests (single-threaded):**
  ```sh
  make test-integration
  ```
- **Clean embedding model cache:**
  ```sh
  make clean-cache
  ```

### Troubleshooting

- **If you see `Too many open files` errors:**
  - Run `ulimit -n 4096` in your shell before running tests.
  - You may also reduce test parallelism: `cargo test -- --test-threads=1`
- **If you see `.lock` errors:**
  - Run `make clean-cache` to remove cache and try again.
- **Note:**
  - The Makefile provides handy shortcuts for common tasks, but some OS or CI environments may require manual adjustment of file descriptor limits (`ulimit`).
  - See each Makefile target for details.

---

## Task Management & Progress

For up-to-date milestones, progress, and detailed task tracking, see:
➡️ [mc-mcp Task List](.amf/task/task-list.md)

---

## License

[MIT](LICENSE)


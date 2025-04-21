# mc-mcp

A Model Context Protocol (MCP) server for the [metacontract (mc)](https://github.com/metacontract/mc) framework, providing smart contract development support via Foundry integration and semantic documentation search.

---

## Overview

**mc-mcp** is an extensible MCP server designed for the [metacontract](https://github.com/metacontract/mc) (mc) framework.
It enables AI-powered smart contract development workflows by exposing tools such as:

- **`mc_search_docs`**: Perform semantic search over mc documentation and user-configured sources, returning structured JSON results.
- **`mc_test`**: Run Foundry tests (`forge test`) in your workspace.
- **`mc_setup`**: Initialize a new Foundry project using the `metacontract/template` (requires an empty directory).
- **`mc_deploy`**: Deploy contracts using the script specified in `mcp_config.toml`. Supports dry-run (default) and broadcast mode (`broadcast: true` argument).
- **`mc_upgrade`**: Upgrade contracts using the script specified in `mcp_config.toml`. Supports dry-run and broadcast mode.
- **`mc_lint`**: (Coming Soon) Lint project files for best practices and errors.

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

## Configuration and Project Root

**Project Root is always specified by the `MC_PROJECT_ROOT` environment variable.**

- Set `MC_PROJECT_ROOT` to your project root directory before running any mc-mcp command or tool.
- All configuration, including `mcp_config.toml`, is expected to be in this directory.
- If `MC_PROJECT_ROOT` is not set or does not exist, mc-mcp will return an error and not start.
- If `mcp_config.toml` is missing, mc-mcp will use built-in defaults and log a warning.

### Example: Setting up and running mc-mcp

```sh
export MC_PROJECT_ROOT=/path/to/your/project
mc-mcp
```

### Example: Project Structure

```
/path/to/your/project/
├── mcp_config.toml   # (optional) Project configuration
├── contracts/
├── scripts/
└── ...
```

### Configuration File (`mcp_config.toml`)
- Place this file directly under your `MC_PROJECT_ROOT` directory.
- If omitted, mc-mcp will use default settings for all configuration values.
- See below for a sample config and field descriptions.

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
# Reference sources for semantic search
[reference]
# The prebuilt index is downloaded to the system cache directory by default.
# You can override the path here if needed:
# prebuilt_index_path = "/custom/path/to/prebuilt_index.jsonl.gz"

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

# Default scripts used by tools
[scripts]
deploy = "scripts/Deploy.s.sol"  # Used by mc_deploy
upgrade = "scripts/Upgrade.s.sol" # Used by mc_upgrade

# Optional: Settings for broadcasting transactions (used when broadcast=true)
rpc_url = "http://localhost:8545" # RPC endpoint URL
private_key_env_var = "PRIVATE_KEY" # Name of the env var holding the deployer's private key
```

- `prebuilt_index_path` ... (optional) Path to a prebuilt index (jsonl or gzipped jsonl). If set, it overrides the default cache location. The index will be loaded and upserted into Qdrant on startup.
- Each `[[reference.sources]]` must have `name`, `source_type` (usually `local`), and `path` (relative to the execution directory).
- `[scripts].deploy` and `[scripts].upgrade` specify the default Foundry script paths used by the `mc_deploy` and `mc_upgrade` tools respectively.
- `[scripts].rpc_url` and `[scripts].private_key_env_var` are required when using `mc_deploy` or `mc_upgrade` with the `broadcast: true` argument. `forge` will use these to send the transaction.
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
- **Integration tests**: Use [testcontainers](https://github.com/testcontainers/testcontainers-rs) for Qdrant, ensuring isolated and stable test environments.
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
  - Increase the file descriptor limit in your shell *before* running tests: `ulimit -n 4096`
  - Run tests sequentially: `cargo test -- --test-threads=1` (or use `make test-lib` / `make test-integration`)
- **If you see `.lock` errors (related to embedding model cache):**
  - Clean the cache: `make clean-cache`
- **If tests involving `forge` commands fail unexpectedly:**
  - Ensure the mock script setup within the specific test file (`src/main.rs` tests) is correct.
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


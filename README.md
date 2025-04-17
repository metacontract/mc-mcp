# mc-mcp

A Model Context Protocol (MCP) server for the [metacontract (mc)](https://github.com/metacontract/mc) framework, providing smart contract development support via Foundry integration and semantic documentation search.

---

## Overview

**mc-mcp** is an extensible MCP server designed for the [metacontract](https://github.com/metacontract/mc) (mc) framework.
It enables AI-powered smart contract development workflows by exposing tools such as:

- **`forge_test`**: Run Foundry tests (`forge test`) in your workspace.
- **`search_docs`**: Perform semantic search over mc documentation and user-configured sources, returning structured JSON results.

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
- Design, search docs, and run TDD cycles (`forge_test`, `search_docs`, etc.)

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

---

## Task Management & Progress

For up-to-date milestones, progress, and detailed task tracking, see:
➡️ [mc-mcp Task List](.amf/task/task-list.md)

---

## License

[MIT](LICENSE)


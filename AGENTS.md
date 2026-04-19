# Repository Guidelines

## Project Structure

This project is a single-binary Rust crate (an MCP server for Swiss pollen data).

- **`src/main.rs`** — Entry point, HTTP layer, request routing, and integration tests.
- **`src/mcp_engine.rs`** — Core MCP protocol logic: request/response types, error handling, CSV parsing, and tool definitions.
- **`tests/fixtures/`** — Test fixtures for integration tests.
- **`Cargo.toml`** — Package manifest and dependency list.
- **`pollen_recent.csv`** — Sample data file for reference.

## Build, Test, and Development

| Command | Description |
| --- | --- |
| `cargo build` | Compile the project. |
| `cargo run` | Run the MCP server (reads JSON-RPC requests from stdin). |
| `cargo test` | Run all unit tests. |
| `cargo test -- --nocapture` | Run tests with output printed. |
| `cargo clippy` | Run the linter. |
| `cargo fmt` | Format all source files. |

The server communicates over JSON-RPC via stdin/stdout. There is no HTTP server — it is a stdio-based MCP transport.

## Coding Style

- **Edition 2021**, Rust stable.
- Follow standard Rust conventions: `snake_case` for functions/variables, `PascalCase` for types, `SCREAMING_SNAKE_CASE` for constants.
- Run `cargo fmt` and `cargo clippy` before submitting changes.
- Keep structs and types in the module (`mcp_engine.rs`) where they are primarily used. Avoid duplicating types across modules.
- Do not add `dead_code` allowances without justification. Prefer removing unused code.

## Testing

- Tests live alongside the code in `src/main.rs` under `#[cfg(test)]` modules.
- Use `cargo test` to run. All tests should pass with zero warnings before committing.
- Integration test fixtures go in `tests/fixtures/`.

## Commit & Pull Request Guidelines

- Write clear, imperative commit messages (e.g., "Fix CSV parsing for empty fields").
- Keep commits focused: one logical change per commit.
- Pull requests should include a description of what changed and why, plus any relevant test updates.

## Architecture

The server is a JSON-RPC 2.0 MCP server over stdio:

1. **`main()`** reads lines from stdin, deserializes each as an `McpRequest`.
2. **`handle_request()`** routes to the appropriate handler (`initialize`, `tools/list`, `tools/call`).
3. **`mcp_engine.rs`** provides the protocol types, tool definitions, and shared utilities (CSV parsing, error formatting).
4. Business logic (`list_pollen_stations`, `get_pollens`) calls external Swiss data APIs (geo.admin.ch).

## Agent Tool Guidelines

- **Search codebase** — Use `fff` MCP server for all file and content searches.
- **Run GitLab pipelines** — Use `opal` MCP server to execute and monitor pipelines.
- **Verify correctness** — Use `context7` MCP server to confirm library, framework, and API behavior.
- **Use available skills** — Apply `rust` skills and any other defined MCP skills whenever relevant.

# DEVFORGE Native Architecture

DEVFORGE is moving toward a native-first architecture:

- Rust: core workspace/file parsing, indexing, process/tool orchestration, LSP lifecycle and security boundaries.
- GPU UI: renderer abstraction is reserved for a future GPUI/Vello migration; the existing Tauri shell remains the compatibility UI in this milestone.
- LSP: standard language-server processes are reused instead of reimplementing language intelligence.
- Extensions: WebAssembly/Wasmtime is an optional sandboxed runtime. Extensions declare permissions and must not receive ambient OS access.
- VS Code/Copilot: external tools remain integrations; GitHub authentication stays owned by GitHub/Copilot CLI.

## Design goal

Editor hot paths should remain independent of JavaScript so large-file parsing, workspace indexing and tool orchestration can move to Rust without blocking the UI.

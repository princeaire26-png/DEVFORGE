# MILESTONE 12 — RUST NATIVE CORE / LSP / EXTENSION ARCHITECTURE

Implemented:
- Rust-native core module for language detection and workspace indexing.
- Native LSP availability detection for Rust, TypeScript/JavaScript, Python, Go, C/C++, C#, Java, Kotlin and Zig.
- Workspace indexer excludes generated/dependency folders and identifies common project manifests.
- Native language-info and workspace-index Tauri commands exposed to the IDE.
- Wasmtime-backed extension-host feature scaffold (`wasm-extensions`) with module validation and isolated Store creation.
- Explicit extension manifest model with permission declarations.
- Architecture boundary separates high-performance native core from the current desktop UI shell.

Important architecture note:
The current product shell remains Tauri + web UI for compatibility and fast iteration. This milestone moves the performance-sensitive editor services into Rust. A complete GPUI/Vello renderer replacement is intentionally not claimed yet; it is a subsequent renderer migration and requires native GPU build/verification.

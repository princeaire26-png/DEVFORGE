# DEVFORGE Milestone 15 — Approval-Gated AI Codebase Agent

Release metadata for the source archive is `1.5.1` in `package.json`, `package-lock.json`, `src-tauri/Cargo.toml`, and `src-tauri/tauri.conf.json`. The desktop build has not been verified in this environment.

## Workflow

1. Open a workspace in the native IDE and authenticate with the official GitHub Copilot CLI.
2. Enter a natural-language feature request or bug report.
3. DEVFORGE indexes the workspace, gathers relevant source and project-instruction files, and sends bounded context to Copilot. Workspace symlinks are not followed.
4. Copilot returns a structured JSON proposal: summary, risks, exact per-file changes, and suggested fixed build/test profiles. DEVFORGE does not accept an unavailable/error response as a successful plan.
5. Review the files, snippets, replacement diff, risks, and validation selections. Rejecting a plan clears it without writing.
6. Approve explicitly. A one-time native approval token is bound to the canonical workspace and exact patch data. The application verifies each file revision and exact snippet, creates backups, then applies the validated text-only edits.
7. Review the changed-file report and diffs. Backups can be restored if the target files have not subsequently changed.
8. Choose an explicitly confirmed build/test profile. Review captured stdout, stderr, exit code, duration, timeout status, and propose any repair through Copilot. Repair plans go through the same review and approval flow.

## Patch and write constraints

- Existing text files only; at most 12 files in one approved plan.
- Workspace-relative paths only; reject traversal, absolute paths, protected/generated directories, out-of-workspace symlinks, invalid UTF-8, binary-looking files, duplicate targets, and ambiguous/non-unique snippets.
- Patch file revisions must still match the content gathered during analysis and are rechecked before writing.
- All patches validate before writes start. Original contents are backed up under `.devforge/backups/agent-<unique-stamp>/`.
- Restore verifies both the backup path and that the edited target still has the expected post-apply content. Later manual edits are never silently overwritten.
- Applying is not transactional across hardware/filesystem failure; a best-effort rollback is attempted and failures are reported.
- The editor's explicit Save and terminal features remain separate user-directed tools.

## Command and repair boundaries

- Only fixed profiles are exposed by the agent: `npm-build`, `npm-test`, `cargo-check`, `cargo-test`, and `python-test`.
- npm profiles require corresponding scripts in `package.json`; Cargo profiles require a recognized manifest; Python test requires a recognized Python project/test manifest.
- Each command requires user confirmation, has a 180-second timeout, and retains bounded stdout/stderr. Workspace scripts are executable code and may launch child processes or access the host.
- Timeout termination targets the direct command process; DEVFORGE does not use Windows Job Objects or another process-tree containment mechanism, so a workspace-spawned child may outlive the timeout.
- DEVFORGE does not execute shell commands proposed by Copilot and does not claim OS-level isolation.
- Automatic native marker repair has been removed. A failure is diagnostic input to a proposed Copilot repair; file changes require a new explicit approval and validation is manually re-run.
- GitHub credentials are managed only by GitHub's Copilot CLI login. DEVFORGE does not ask for or store passwords.

## Implementation status and limitations

- The native IDE entry point is `client/src/main.tsx`; the native commands and patch engine are in `src-tauri/src/lib.rs`; codebase indexing is in `src-tauri/src/core/mod.rs`.
- Copilot planning requires the official Copilot CLI to be installed and authenticated. If it is unavailable, DEVFORGE reports an error instead of fabricating an AI plan.
- Context retrieval uses bounded lexical matching, symbol/import metadata, entry points, and instruction-file preference. It is heuristic, not compiler-grade semantic search, and can omit relevant context.
- Patch operations use exact one-time snippet replacement, not an AST-aware transform. Review every diff and use version control.
- This implementation is not a secure multi-user authorization boundary. The explicit native approval API prevents accidental/unreviewed writes in the app workflow; it is not protection against a compromised application process or local user.
- Build/test execution is not sandboxed. Only run commands in workspaces you trust.

## Focused tests

Rust unit tests cover traversal/protected paths, malformed and ambiguous patches, binary content, stale revisions, one-time approval, backup recovery/rejection after later edits, and timeout behavior:

```powershell
cargo test --manifest-path src-tauri\Cargo.toml
```

Frontend/backend checks:

```powershell
npm.cmd run check
npm.cmd run build
npm.cmd test
```

## VS Code handoff

See [VSCODE_HANDOFF.md](./VSCODE_HANDOFF.md). This repository source is not a verified Windows installer; build and test the native application on the target Windows machine.

## Tauri Windows packaging configuration

- Vite writes the frontend to `dist/client`; Tauri's `frontendDist` is `../dist/client` relative to `src-tauri/`.
- Release targets are NSIS and MSI. The WebView install mode is `downloadBootstrapper`.
- The app has a source SVG under `src-tauri/icons/`, but the Tauri `bundle.icon` list is empty, so that SVG is not configured as a Windows bundle icon.
- The `default` capability enables core and dialog plugin defaults. Application commands are registered in the Tauri invoke handler; frontend command-name and Rust-definition cross-check passed for the 46 unique frontend invocation names.
- Rust, Cargo, Visual Studio Build Tools with MSVC, and a Windows SDK are prerequisites for compilation. This environment's Tauri readiness report found Rust/Cargo and MSVC/SDK missing. WebView2 Runtime was detected, but no desktop binary or installer has been built or tested here.
- No `Cargo.lock` is present in the source snapshot because Cargo is unavailable here. Generate it and run the Rust checks before calling the desktop build reproducible.

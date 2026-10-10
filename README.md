# DEVFORGE v1.5.1

**From idea to software.**

DEVFORGE is an AI-native software factory prototype. The first vertical slice is:

**IDEA → PLAN → APPROVAL → BUILD → VALIDATION → PREVIEW**

## Run

```bash
npm install
npm run dev
```

Open the Vite URL shown by the dev server. The Express API runs on port 8787 by default.

## AI modes

The idea-to-blueprint web workflow is deterministic demo output. Setting `DEVFORGE_AI_MODE` to anything other than `demo` returns an explicit unavailable-provider error; it does not relabel demo output as AI-generated.

The native IDE's AI Codebase Agent uses the official GitHub Copilot CLI after the user signs in through Copilot. It does not request or store GitHub passwords. Copilot analysis and planning do not write files; application requires a separate review and approval.

## Architecture

- `client/` React + Vite UI
- `server/` Express API, SQLite persistence, planning/build orchestration
- `shared/` future shared types
- `tests/` smoke tests

## Safety

Build/test commands execute code from the opened workspace. They are fixed, manifest-checked profiles with explicit confirmation, bounded output, and timeouts; they are **not an OS-level sandbox**. Timeouts terminate the direct command process, not a Windows Job Object/process tree, so child processes may outlive a timed-out command. Do not run checks or open terminals on workspaces you do not trust.

## Native Windows application

DEVFORGE includes a Tauri 2 desktop shell under `src-tauri/`.

- `npm run dev:desktop` — launch the native desktop shell during development.
- `npm.cmd run build:desktop` — attempt Windows NSIS/MSI bundles when built on Windows with Rust, MSVC C++ tools, Windows SDK, and WebView2 prerequisites installed.

The current native shell packages the React UI as a Windows desktop WebView. The application still includes a Node/Express API for its original project workflow; the native IDE workflow runs through Tauri commands.

### Native standalone behavior

The desktop UI has a local-storage fallback for the project/plan/build workflow when the Node development API is not running. These fallback plans are explicitly marked `DEMO-NATIVE`; they are not presented as real AI generation. The Codebase Agent itself requires the Tauri native runtime and authenticated Copilot CLI.

## DEVFORGE v0.3 milestone — Native project generation

The Windows/Tauri runtime now exposes native commands for creating and inspecting DEVFORGE project workspaces. When running inside Tauri, approving a plan creates a real project directory under the application's local data directory, including a README, starter TypeScript source, build-plan document, and `devforge.project.json` manifest.

This is the first native project-generation milestone. The IDE also has terminal and debug commands. Workspace code executes on the host; DEVFORGE does not provide an OS-level sandbox. Use only trusted workspaces and inspect the safety boundaries in `MILESTONE_15.md`.


## Milestone 05
DEVFORGE includes native project validation, repair history, and a clearly labeled controlled failure simulator. Validation does not automatically modify source files; proposed repairs must go through the Milestone 15 approval flow. See `MILESTONE_05.md`.


## Native IDE capabilities
DEVFORGE now includes a native workspace explorer/editor, search, terminal execution, external terminal launch, Git status, and GitHub Copilot CLI integration. Copilot uses GitHub's official CLI authentication and permission model rather than DEVFORGE storing GitHub credentials.


## Milestone 9 — Debugger
DEVFORGE now includes a native debugger session foundation with process lifecycle, streaming output, breakpoint state, and debugger-adapter detection. True pause/step/variables require a DAP adapter and are intentionally not simulated.


## Milestone 15 — AI Codebase Agent

The native IDE includes a Copilot-assisted workflow for analyzing an open codebase and proposing exact multi-file edits. Native writes require a one-time approval bound to the exact patch plan; existing target revisions are checked, backups are created first, and guarded restore is available. Build and test commands require confirmation and never launch model-proposed shell strings. Failures can be sent back to Copilot as repair requests, but generated repairs still require review and approval.

See [MILESTONE_15.md](./MILESTONE_15.md) for implementation scope and limitations and [VSCODE_HANDOFF.md](./VSCODE_HANDOFF.md) for exact setup, build, test, and debugger commands. Do not claim a Windows installer works until it is built and exercised on Windows with the required native toolchain.

## Release source archive

The `DEVFORGE-v1.5.1-AI-CODEBASE-AGENT.zip` source archive excludes dependencies, generated build output, local databases, and environment files. Its internal `RELEASE_MANIFEST.md` records included paths and the checks run for this source release. The source archive is not a Windows installer. Rust compilation is not verified and no `Cargo.lock` is present; generate and verify one with Cargo before treating Windows packaging as reproducible.

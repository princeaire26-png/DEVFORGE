# DEVFORGE Milestone 07 — Universal Workspace + GitHub Authentication Gate

Implemented in the native IDE layer:

- Open arbitrary text/source files from the Windows file picker, not only DEVFORGE project files.
- Save arbitrary opened text/source files with an 8 MB safety limit.
- Binary-file detection prevents accidental text editing of binary files.
- Language detection for a broad set of languages and configuration formats including JavaScript, TypeScript, JSX/TSX, Python, Rust, Go, Java, Kotlin, Swift, C/C++, C#, PHP, Ruby, Dart, Lua, R, Scala, Elixir, Erlang, Haskell, F#, SQL, Shell, PowerShell, Batch, Assembly, Solidity, Vue, Svelte, Astro, HTML, CSS/SCSS/Sass/Less, JSON, XML, YAML, TOML, Markdown, GraphQL, Protobuf, Dockerfile, Terraform and more.
- GitHub Copilot is explicitly locked until the user authenticates through the official Copilot CLI GitHub login flow.
- Added a DEVFORGE `Login with GitHub` action that launches `copilot login` in the project terminal. GitHub's official Copilot CLI uses OAuth in a local desktop flow and stores credentials in the Windows Credential Manager when available.
- Copilot connection status is checked before enabling the Ask Copilot action.
- Copilot requests continue to run against the currently opened project workspace.
- Existing native terminal, Git status, repair engine and project editor remain available.

## Important scope

DEVFORGE does not claim to reproduce every VS Code feature yet. This milestone establishes universal file access, language awareness, and a correct GitHub/Copilot authentication gate. Full language intelligence will be expanded with LSP servers, diagnostics, autocomplete and debugging in later milestones.

## Verification

The source was updated successfully. Full npm/Tauri compilation could not be completed in the available environment because `npm install --ignore-scripts --no-audit --no-fund` exceeded the 120-second execution limit and a Rust/Windows toolchain was not available for a complete native build.

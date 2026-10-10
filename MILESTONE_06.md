# DEVFORGE MILESTONE 06 — NATIVE IDE + TERMINAL + GITHUB COPILOT

Implemented:
- Native project file explorer backed by the Rust runtime.
- Workspace file read/write with project-root containment and size limits.
- Workspace text search.
- Integrated terminal command execution from the native project directory with timeout/output capture.
- External native terminal launcher.
- Git status integration.
- GitHub Copilot CLI integration through the official `copilot -sp` programmatic interface.
- Copilot connection/status panel and a one-click external Copilot terminal.
- IDE-style editor, terminal, source-control, agents and Copilot panels.
- Six visible agent roles: Orchestrator, Architect, Coder, Tester, Security, Deployer.

## Copilot authentication
DEVFORGE does not impersonate GitHub authentication or store Copilot credentials. The native app delegates authentication and permissions to GitHub Copilot CLI. The user can authenticate with `/login` in Copilot CLI and then use DEVFORGE's Copilot panel.

## Important scope
This milestone provides the core native IDE toolchain rather than claiming pixel-for-pixel parity with every Visual Studio Code feature. The architecture is ready for additional language servers, debugging, extensions, task runners, Git operations, and richer editor services.

## Safety
Terminal commands execute with the user's Windows account permissions. DEVFORGE shows the command and captures its output, but this is not an OS sandbox. Future sandboxing will isolate AI-driven execution from the host system.

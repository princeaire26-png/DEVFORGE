# DEVFORGE Milestone 10 — Real DAP Debugger

Implemented a real Debug Adapter Protocol client foundation in the native Tauri runtime.

## Included
- DAP process launching over stdin/stdout with Content-Length framing.
- DAP initialize handshake.
- Launch/configurationDone lifecycle.
- Breakpoint requests.
- Continue, pause, step-over, step-into, step-out.
- Threads, stackTrace, scopes, variables and evaluate requests.
- DAP session lifecycle and stop.
- Adapter command discovery with language-specific environment overrides.
- Python debugpy adapter, Go Delve, and CodeLLDB defaults where installed.
- Generic DEVFORGE_DAP_*_COMMAND overrides for additional adapters.

## Safety
DEVFORGE does not silently execute an arbitrary debugger binary. Adapter selection is explicit by language or environment configuration, and the debug process runs in the selected workspace.

## Limitation
The current UI still needs the next pass to render full threads/stack/scopes/variables and consume DAP events asynchronously. The native protocol client is now present; this milestone is the runtime foundation for the full debugger UI.

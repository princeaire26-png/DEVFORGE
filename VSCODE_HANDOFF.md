# DEVFORGE — VS Code Setup and Verification (Windows)

This repository is source code, not a verified Windows executable. Complete the checks on the Windows machine that will run or package it.

## Prerequisites

- VS Code and Git for Windows.
- Node.js LTS (npm).
- Rust stable (`rustup`/`cargo`).
- Visual Studio Build Tools with **Desktop development with C++** and a Windows SDK.
- Tauri 2 Windows prerequisites, including WebView2.
- Optional: GitHub Copilot CLI, installed and authenticated using its official workflow and an eligible GitHub Copilot subscription.

## Install the missing native prerequisites

Use these Windows GUI installers where available:

1. **Visual Studio Build Tools:** download and run the installer from [Visual Studio Build Tools](https://aka.ms/vs/17/release/vs_BuildTools.exe). In Visual Studio Installer choose **Modify** or **Install**, select **Desktop development with C++**, and ensure a Windows 10/11 SDK is selected under Installation details. Complete setup and restart VS Code.
2. **WebView2:** install the Evergreen Runtime from the [Microsoft Edge WebView2 download page](https://developer.microsoft.com/microsoft-edge/webview2/). Tauri's current readiness check detected WebView2 on this machine.
3. **Rust/Cargo:** download the official `rustup-init.exe` from [rustup.rs](https://rustup.rs/) and run it. The Windows installer is a text-mode setup (Rust does not provide a standard Windows GUI installer); accept the default stable MSVC toolchain. Reopen PowerShell/VS Code after installation.
4. Verify in the VS Code integrated terminal:

   ```powershell
   rustc --version
   cargo --version
   cargo generate-lockfile --manifest-path src-tauri\Cargo.toml
   cargo check --manifest-path src-tauri\Cargo.toml
   cargo test --manifest-path src-tauri\Cargo.toml
   ```

   Review and retain the generated `src-tauri\Cargo.lock` in source control. If Rust reports that the MSVC linker or Windows SDK is missing, return to Visual Studio Installer and verify the C++ workload and SDK are installed.

## Open and install

In VS Code, open the repository root folder, then run these commands in the integrated PowerShell terminal:

```powershell
npm.cmd install
npm.cmd run check
npm.cmd run build
npm.cmd test
cargo check --manifest-path src-tauri\Cargo.toml
cargo test --manifest-path src-tauri\Cargo.toml
```

Use `npm.cmd` in PowerShell if local execution policy blocks `npm.ps1`. Do not bypass the execution policy.

## Run the application

Browser development (React/Vite plus Express API):

```powershell
npm.cmd run dev
```

Native desktop development:

```powershell
npm.cmd run dev:desktop
```

The app includes the original Express workflow and local demo fallback. The AI Codebase Agent is in the native IDE and requires the Copilot CLI to be installed and authenticated. Without Copilot, planning is unavailable; DEVFORGE must not represent demo output as AI output.

## Build the desktop bundle

```powershell
npm.cmd run build:desktop
```

This only attempts a platform build. Report an installer as working only after the command succeeds and the generated application has been installed/launched and exercised on Windows.

## Verify the approval-gated agent

Use a disposable Git repository with a clean baseline:

1. Open the workspace from the IDE.
2. Click **Login with GitHub**, complete the official Copilot login, and **Check connection**.
3. Enter a small feature or bug-fix request and plan it.
4. Confirm the analysis lists relevant files. Review summary, risks, every proposed before/after diff, and fixed validation profile.
5. Reject once and confirm there are no writes. Plan again, approve, then inspect changed files and `.devforge/backups/`.
6. Edit one changed file after apply and confirm **Restore backups** refuses to overwrite that later edit. Restore an untouched test case and verify original contents return.
7. Run the proposed build/test profiles; confirm each requires confirmation and shows actual exit status and captured output. When a check fails, request a proposed repair and approve its patch separately.
8. Try malformed JSON, an absolute/traversal path, a duplicate or non-unique snippet, and a stale patch revision; each must be rejected without writes.

Build/test commands execute workspace code on the host. DEVFORGE timeouts target the direct command process only and do not guarantee its child processes are terminated. Allowlisted profiles and timeouts are not an OS sandbox.

## Debugging in VS Code

For native Rust command failures:

```powershell
cargo test --manifest-path src-tauri\Cargo.toml -- --nocapture
cargo check --manifest-path src-tauri\Cargo.toml
```

Open `src-tauri\src\lib.rs` or `src-tauri\src\core\mod.rs`, set breakpoints, and use the Rust Analyzer extension's debugger integration if configured locally. For Tauri UI failures, run `npm.cmd run dev:desktop` from the integrated terminal and use the WebView devtools. Debug adapter availability depends on local VS Code extensions/toolchain configuration.

## Reporting the result

Record exact command output and distinguish:

- passed checks;
- checks blocked by missing Node/Rust/C++/WebView2/Copilot prerequisites;
- actual runtime or build failures;
- native installer status (not built, built, or installed/launched and tested).

Do not claim build, sandbox, debugger, or Copilot behavior that was not exercised.

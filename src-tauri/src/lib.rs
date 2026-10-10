mod core;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fs, hash::{Hash, Hasher}, io::{BufRead, BufReader, Read}, path::{Component, Path, PathBuf}, process::{Child, Command, ExitStatus, Stdio}, sync::{Arc, Mutex}, thread, time::{Duration, Instant}};
use tauri::Manager;

struct DebugSession {
    child: Child,
    command: String,
    cwd: String,
    language: String,
    breakpoints: Vec<String>,
    output: Arc<Mutex<String>>,
    started_at: Instant,
}

struct DapSession {
    child: Child,
    seq: u64,
    initialized: bool,
}

#[derive(Default)]
struct AppState {
    debug: Mutex<HashMap<u64, DebugSession>>,
    dap: Mutex<HashMap<u64, DapSession>>,
    agent_approvals: Mutex<HashMap<String, AgentApproval>>,
}

struct AgentApproval {
    root: PathBuf,
    plan_digest: String,
    expires_at: Instant,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NativeProject { pub id: String, pub name: String, pub root: String }

#[derive(Debug, Serialize, Deserialize)]
pub struct BuildCommandResult {
    pub profile: String,
    pub status: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u128,
}

struct CapturedCommand {
    status: Option<ExitStatus>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    timed_out: bool,
    duration_ms: u128,
}

fn read_bounded(mut stream: impl Read) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let count = stream.read(&mut buffer)?;
        if count == 0 { break; }
        let remaining = 65_536usize.saturating_sub(output.len());
        output.extend_from_slice(&buffer[..count.min(remaining)]);
    }
    Ok(output)
}

fn capture_command(program: &str, args: &[&str], cwd: &Path, timeout: Duration) -> Result<CapturedCommand, String> {
    let started = Instant::now();
    let mut child = Command::new(program).args(args).current_dir(cwd).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().map_err(|e| format!("Could not start {program}; confirm it is installed and on PATH: {e}"))?;
    let stdout = child.stdout.take().ok_or_else(|| "Could not capture command stdout.".to_string())?;
    let stderr = child.stderr.take().ok_or_else(|| "Could not capture command stderr.".to_string())?;
    let stdout_reader = thread::spawn(move || read_bounded(stdout));
    let stderr_reader = thread::spawn(move || read_bounded(stderr));
    let mut timed_out = false;
    loop {
        if child.try_wait().map_err(|e| format!("Could not poll {program}: {e}"))?.is_some() { break; }
        if started.elapsed() >= timeout {
            timed_out = true;
            child.kill().map_err(|e| format!("Could not stop timed-out {program}: {e}"))?;
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    let status = child.wait().map_err(|e| format!("Could not collect {program} exit status: {e}"))?;
    let stdout = stdout_reader.join().map_err(|_| "Command stdout reader stopped unexpectedly.".to_string())?
        .map_err(|e| format!("Could not read command stdout: {e}"))?;
    let stderr = stderr_reader.join().map_err(|_| "Command stderr reader stopped unexpectedly.".to_string())?
        .map_err(|e| format!("Could not read command stderr: {e}"))?;
    Ok(CapturedCommand { status: if timed_out { None } else { Some(status) }, stdout, stderr, timed_out, duration_ms: started.elapsed().as_millis() })
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RepairResult {
    pub status: String,
    pub attempts: u32,
    pub patches_applied: Vec<String>,
    pub build: Option<BuildCommandResult>,
    pub test: Option<BuildCommandResult>,
    pub log_path: String,
}

fn safe_slug(input: &str) -> String {
    let mut out = String::new();
    for c in input.chars() {
        if c.is_ascii_alphanumeric() { out.push(c.to_ascii_lowercase()); }
        else if (c == ' ' || c == '-' || c == '_') && !out.ends_with('-') { out.push('-'); }
    }
    out.trim_matches('-').chars().take(48).collect()
}

fn write_if_missing(path: &Path, content: &str) -> Result<(), String> {
    if !path.exists() { fs::write(path, content).map_err(|e| e.to_string())?; }
    Ok(())
}

fn projects_base(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let base = app.path().app_data_dir().map_err(|e| e.to_string())?.join("projects");
    fs::create_dir_all(&base).map_err(|e| e.to_string())?;
    Ok(base)
}

fn safe_project_root(app: &tauri::AppHandle, root: &str) -> Result<PathBuf, String> {
    let base = projects_base(app)?.canonicalize().map_err(|e| e.to_string())?;
    let requested = PathBuf::from(root).canonicalize().map_err(|e| format!("Invalid project path: {e}"))?;
    if requested.starts_with(&base) { Ok(requested) } else { Err("Project path is outside the DEVFORGE workspace.".into()) }
}



#[derive(Debug, Clone, Serialize, Deserialize)]
struct AgentPatch { path: String, find: String, replace: String, reason: String, expected_hash: String }

#[derive(Debug, Serialize, Deserialize)]
struct AgentBackup { path: String, expected_hash: String }

#[derive(Debug, Serialize, Deserialize)]
struct AgentApplyResult { changed: Vec<String>, backups: Vec<AgentBackup>, message: String }

fn agent_text_hash(text: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn agent_plan_digest(patches: &[AgentPatch]) -> Result<String, String> {
    let json = serde_json::to_string(patches).map_err(|e| format!("Could not encode approved patch plan: {e}"))?;
    Ok(agent_text_hash(&json))
}

fn validate_agent_relative_path(path: &str) -> Result<PathBuf, String> {
    let rel = PathBuf::from(path);
    if path.trim().is_empty() || path.contains('\0') || path.contains(':') || rel.is_absolute() {
        return Err(format!("Unsafe relative path: {path}"));
    }
    let mut has_component = false;
    for component in rel.components() {
        match component {
            Component::Normal(name) => {
                let name = name.to_string_lossy().to_ascii_lowercase();
                if [".git", "node_modules", "target", "dist", "build", ".devforge"].contains(&name.as_str()) {
                    return Err(format!("Protected/generated paths cannot be edited by the agent: {path}"));
                }
                has_component = true;
            }
            _ => return Err(format!("Unsafe relative path: {path}")),
        }
    }
    if !has_component { return Err(format!("Unsafe relative path: {path}")); }
    Ok(rel)
}

fn create_agent_backup_root(base: &Path, stamp: u128) -> Result<PathBuf, String> {
    let mut directory = base.to_path_buf();
    for name in [".devforge".to_string(), "backups".to_string(), format!("agent-{stamp}")] {
        directory.push(name);
        match fs::symlink_metadata(&directory) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() =>
                return Err(format!("Backup location is not a safe directory: {}", directory.display())),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound =>
                fs::create_dir(&directory).map_err(|e| format!("Could not create backup directory {}: {e}", directory.display()))?,
            Err(error) => return Err(format!("Could not inspect backup directory {}: {error}", directory.display())),
        }
    }
    let canonical = directory.canonicalize().map_err(|e| format!("Could not validate backup directory: {e}"))?;
    if !canonical.starts_with(base) { return Err("Backup directory escapes the workspace.".into()); }
    Ok(canonical)
}

fn apply_agent_patch_text(original: &str, patch: &AgentPatch) -> Result<String, String> {
    if patch.find.is_empty() || patch.find.len() > 32_000 || patch.replace.len() > 64_000 {
        return Err(format!("Patch text is empty or exceeds safety limits for {}.", patch.path));
    }
    if original.len() > 8 * 1024 * 1024 || original.as_bytes().contains(&0) {
        return Err(format!("{} is too large or appears to be a binary file.", patch.path));
    }
    if patch.expected_hash != agent_text_hash(original) {
        return Err(format!("Patch rejected for {}: file changed after planning. Re-plan against the current contents.", patch.path));
    }
    let occurrences = original.matches(&patch.find).count();
    if occurrences != 1 {
        return Err(format!("Patch rejected for {}: expected exact snippet once, found {} times.", patch.path, occurrences));
    }
    let updated = original.replacen(&patch.find, &patch.replace, 1);
    if updated.len() > 8 * 1024 * 1024 || updated.as_bytes().contains(&0) {
        return Err(format!("Updated file is too large or appears to be binary: {}", patch.path));
    }
    Ok(updated)
}

fn consume_agent_approval(state: &AppState, token: &str, root: &Path, patches: &[AgentPatch]) -> Result<(), String> {
    let mut approvals = state.agent_approvals.lock().map_err(|_| "Agent approval state is unavailable.".to_string())?;
    let approval = approvals.remove(token).ok_or_else(|| "Patch approval is missing or already used. Review and approve the plan again.".to_string())?;
    if approval.expires_at <= Instant::now() {
        return Err("Patch approval expired. Review and approve the plan again.".into());
    }
    if approval.root != root || approval.plan_digest != agent_plan_digest(patches)? {
        return Err("Approved patch plan does not match this workspace or the submitted patches.".into());
    }
    Ok(())
}

#[tauri::command]
fn workspace_agent_context(root: String, request: String) -> Result<serde_json::Value, String> {
    let base = canonical_workspace_root(&root)?;
    let request = request.trim();
    if request.is_empty() { return Err("Describe the change you want before planning.".into()); }
    if request.len() > 4000 { return Err("Request is too long (maximum 4,000 characters).".into()); }
    let report = core::analyze_codebase(&base)?;
    let terms: Vec<String> = request.to_ascii_lowercase().split(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-')
        .filter(|t| t.len() >= 3).map(str::to_string).collect();
    let mut scored: Vec<(i32, String)> = report.files.iter().map(|f| {
        let path_l = f.path.to_ascii_lowercase();
        let mut score = terms.iter().filter(|t| path_l.contains(t.as_str())).count() as i32 * 5;
        for sym in report.symbols.iter().filter(|s| s.path == f.path) {
            if terms.iter().any(|t| sym.name.to_ascii_lowercase().contains(t)) { score += 3; }
        }
        for imp in report.imports.iter().filter(|i| i.path == f.path) {
            if terms.iter().any(|t| imp.target.to_ascii_lowercase().contains(t)) { score += 1; }
        }
        (score, f.path.clone())
    }).collect();
    scored.sort_by(|a,b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let instruction_files = ["AGENTS.md", "CLAUDE.md", "CONTRIBUTING.md", "README.md", ".github/copilot-instructions.md"];
    let mut chosen: Vec<String> = instruction_files.iter()
        .filter(|instruction| report.files.iter().any(|f| f.path == **instruction))
        .map(|instruction| (*instruction).to_string()).take(4).collect();
    for (_, path) in scored.iter().filter(|x| x.0 > 0) {
        if chosen.len() == 8 { break; }
        if !chosen.contains(path) { chosen.push(path.clone()); }
    }
    for ep in report.entry_points.iter().take(2) {
        if chosen.len() == 8 { break; }
        if !chosen.contains(ep) { chosen.push(ep.clone()); }
    }
    let mut files = Vec::new(); let mut total = 0usize;
    for rel in chosen {
        let path = base.join(&rel);
        if let Ok(canonical) = path.canonicalize() {
            if canonical.starts_with(&base) && canonical.is_file() {
                if let Ok(text) = fs::read_to_string(&canonical) {
                    if text.len() < 24_000 && total + text.len() <= 72_000 {
                        total += text.len();
                        files.push(serde_json::json!({"path":rel,"revision":agent_text_hash(&text),"content":text}));
                    }
                }
            }
        }
    }
    let selected_paths: std::collections::HashSet<String> = files.iter()
        .filter_map(|file| file["path"].as_str().map(str::to_string)).collect();
    Ok(serde_json::json!({"root":base.to_string_lossy(),"request":request,"files":files,
        "summary":{"file_count":report.files.len(),"symbol_count":report.symbols.len(),"dependencies":report.dependencies.iter().take(80).map(|d|serde_json::json!({"manager":d.manager,"name":d.name,"version":d.version})).collect::<Vec<_>>(),"entry_points":report.entry_points.iter().take(20).collect::<Vec<_>>(),
            "symbols":report.symbols.iter().filter(|s| selected_paths.contains(&s.path)).take(100).map(|s|serde_json::json!({"path":s.path,"name":s.name,"kind":s.kind,"line":s.line})).collect::<Vec<_>>()},
        "safety":"Planning only. No files were changed. Review every patch and explicitly approve before applying."}))
}

#[tauri::command]
fn workspace_authorize_agent_patches(state: tauri::State<AppState>, root: String, patches: Vec<AgentPatch>) -> Result<String, String> {
    let base = canonical_workspace_root(&root)?;
    if patches.is_empty() { return Err("No patches to apply.".into()); }
    if patches.len() > 12 { return Err("A plan may change at most 12 files per approval.".into()); }
    let token = uuid_like();
    let approval = AgentApproval { root: base, plan_digest: agent_plan_digest(&patches)?, expires_at: Instant::now() + Duration::from_secs(600) };
    let mut approvals = state.agent_approvals.lock().map_err(|_| "Agent approval state is unavailable.".to_string())?;
    approvals.retain(|_, value| value.expires_at > Instant::now());
    approvals.insert(token.clone(), approval);
    Ok(token)
}

#[tauri::command]
fn workspace_apply_agent_patches(state: tauri::State<AppState>, root: String, approval_token: String, patches: Vec<AgentPatch>) -> Result<AgentApplyResult, String> {
    let base = canonical_workspace_root(&root)?;
    if patches.is_empty() { return Err("No patches to apply.".into()); }
    if patches.len() > 12 { return Err("A plan may change at most 12 files per approval.".into()); }
    consume_agent_approval(&state, &approval_token, &base, &patches)?;
    let mut seen = std::collections::HashSet::new();
    let mut prepared: Vec<(String, PathBuf, String, String)> = Vec::new();
    for patch in &patches {
        let rel = validate_agent_relative_path(&patch.path)?;
        if !seen.insert(patch.path.clone()) { return Err(format!("Duplicate patch path: {}. Use one combined patch per file.", patch.path)); }
        let file = base.join(&rel);
        let canonical = file.canonicalize().map_err(|e| format!("Cannot open {}: {e}", patch.path))?;
        if !canonical.starts_with(&base) || !canonical.is_file() { return Err(format!("Patch path escapes workspace or is not a file: {}", patch.path)); }
        let original = fs::read_to_string(&canonical).map_err(|e| format!("{} is not readable UTF-8 text: {e}", patch.path))?;
        let updated = apply_agent_patch_text(&original, patch)?;
        prepared.push((patch.path.clone(), canonical, original, updated));
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let backup_root = create_agent_backup_root(&base, stamp)?;
    let mut backups = Vec::new();
    for (rel, _, original, updated) in &prepared {
        let backup = backup_root.join(validate_agent_relative_path(rel)?);
        if let Some(parent) = backup.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
        fs::write(&backup, original).map_err(|e| format!("Could not create backup for {rel}: {e}"))?;
        backups.push(AgentBackup {
            path: backup.strip_prefix(&base).unwrap_or(&backup).to_string_lossy().replace('\\', "/"),
            expected_hash: agent_text_hash(updated),
        });
    }
    let mut changed: Vec<PathBuf> = Vec::new();
    for (rel, path, original, updated) in &prepared {
        let current = fs::read_to_string(path).map_err(|e| format!("Could not recheck {rel} before writing: {e}"))?;
        if current != *original {
            return Err(format!("Patch rejected for {rel}: file changed while backups were being prepared. No patch was written."));
        }
        if let Err(e) = fs::write(path, updated) {
            for changed_path in &changed {
                if let Some((_, _, original, _)) = prepared.iter().find(|(_, p, _, _)| p == changed_path) { let _ = fs::write(changed_path, original); }
            }
            return Err(format!("Could not write {rel}: {e}. Earlier writes were rolled back where possible."));
        }
        changed.push(path.clone());
    }
    Ok(AgentApplyResult { changed: patches.iter().map(|p| p.path.clone()).collect(), backups,
        message: format!("Applied {} reviewed patch(es). Originals are backed up under .devforge/backups/agent-{stamp}/. Run build and tests before accepting the change.", patches.len()) })
}

#[tauri::command]
fn workspace_restore_agent_backups(root: String, backups: Vec<AgentBackup>) -> Result<Vec<String>, String> {
    let base = canonical_workspace_root(&root)?;
    restore_agent_backups_at(&base, &backups)
}

fn restore_agent_backups_at(base: &Path, backups: &[AgentBackup]) -> Result<Vec<String>, String> {
    if backups.is_empty() || backups.len() > 12 { return Err("Select between 1 and 12 reviewed backup(s) to restore.".into()); }
    let backup_base = base.join(".devforge").join("backups").canonicalize().map_err(|e| format!("Backup directory is unavailable: {e}"))?;
    if !backup_base.starts_with(base) { return Err("Backup directory escapes the workspace.".into()); }
    let mut prepared = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for record in backups {
        let path = PathBuf::from(&record.path);
        let components: Vec<_> = path.components().collect();
        if components.len() < 4
            || components[0] != Component::Normal(".devforge".as_ref())
            || components[1] != Component::Normal("backups".as_ref())
            || !components[2].as_os_str().to_string_lossy().starts_with("agent-")
        {
            return Err(format!("Invalid agent backup path: {}", record.path));
        }
        let target_rel = components[3..].iter().fold(PathBuf::new(), |mut target, component| {
            target.push(component.as_os_str());
            target
        });
        let target_name = target_rel.to_string_lossy().replace('\\', "/");
        validate_agent_relative_path(&target_name)?;
        if !seen.insert(target_name.clone()) { return Err(format!("Duplicate restore target: {target_name}")); }
        let backup = base.join(&path).canonicalize().map_err(|e| format!("Backup is unavailable for {target_name}: {e}"))?;
        if !backup.starts_with(&backup_base) || !backup.is_file() { return Err(format!("Backup path escapes the backup directory: {}", record.path)); }
        let target = base.join(&target_rel).canonicalize().map_err(|e| format!("Restore target is unavailable for {target_name}: {e}"))?;
        if !target.starts_with(&base) || !target.is_file() { return Err(format!("Restore target escapes the workspace: {target_name}")); }
        let current = fs::read_to_string(&target).map_err(|e| format!("Cannot read current {target_name}: {e}"))?;
        if agent_text_hash(&current) != record.expected_hash {
            return Err(format!("Restore rejected for {target_name}: file changed after the approved patch. Review it before restoring."));
        }
        let original = fs::read_to_string(&backup).map_err(|e| format!("Backup for {target_name} is not valid UTF-8 text: {e}"))?;
        if original.as_bytes().contains(&0) { return Err(format!("Backup for {target_name} appears to be binary.")); }
        prepared.push((target_name, target, original));
    }
    let mut restored: Vec<(PathBuf, String)> = Vec::new();
    for (name, target, original) in &prepared {
        let current = fs::read_to_string(target).map_err(|e| format!("Could not recheck {name} before restoring: {e}"))?;
        let expected_hash = backups.iter().find_map(|record| {
            let components: Vec<_> = Path::new(&record.path).components().collect();
            let target_name = components.get(3..).map(|parts| parts.iter().fold(PathBuf::new(), |mut path, component| {
                path.push(component.as_os_str());
                path
            }).to_string_lossy().replace('\\', "/"));
            if target_name.as_deref() == Some(name.as_str()) { Some(record.expected_hash.as_str()) } else { None }
        }).ok_or_else(|| format!("Missing restore revision for {name}"))?;
        if agent_text_hash(&current) != expected_hash {
            return Err(format!("Restore rejected for {name}: file changed while backups were being prepared."));
        }
        if let Err(error) = fs::write(target, original) {
            let rollback_failures: Vec<String> = restored.iter().filter_map(|(path, text)| {
                fs::write(path, text).err().map(|failure| format!("{}: {failure}", path.display()))
            }).collect();
            return Err(if rollback_failures.is_empty() {
                format!("Could not restore {name}: {error}. Earlier restores were rolled back.")
            } else {
                format!("Could not restore {name}: {error}. Rollback also failed for {}", rollback_failures.join(", "))
            });
        }
        restored.push((target.clone(), current));
    }
    Ok(prepared.into_iter().map(|(name, _, _)| name).collect())
}

#[tauri::command]
fn workspace_run_check(root: String, profile: String) -> Result<BuildCommandResult, String> {
    let base = canonical_workspace_root(&root)?;
    let (program, args): (&str, Vec<&str>) = match profile.as_str() {
        "npm-build" | "npm-test" => {
            let manifest_path = base.join("package.json");
            let manifest: serde_json::Value = serde_json::from_slice(&fs::read(&manifest_path)
                .map_err(|e| format!("Cannot read package.json for {profile}: {e}"))?)
                .map_err(|e| format!("package.json is invalid JSON: {e}"))?;
            let script = if profile == "npm-build" { "build" } else { "test" };
            if !manifest.get("scripts").and_then(|s| s.get(script)).and_then(serde_json::Value::as_str).is_some() {
                return Err(format!("The project does not define an npm '{script}' script."));
            }
            if profile == "npm-build" {
                if cfg!(windows) { ("cmd.exe", vec!["/D", "/S", "/C", "npm.cmd run build"]) } else { ("npm", vec!["run", "build"]) }
            } else if cfg!(windows) { ("cmd.exe", vec!["/D", "/S", "/C", "npm.cmd test"]) } else { ("npm", vec!["test"]) }
        },
        "cargo-check" => if base.join("Cargo.toml").exists() { ("cargo", vec!["check"]) } else if base.join("src-tauri").join("Cargo.toml").exists() { ("cargo", vec!["check", "--manifest-path", "src-tauri/Cargo.toml"]) } else { return Err("No Cargo.toml found in workspace root or src-tauri/.".into()); },
        "cargo-test" => if base.join("Cargo.toml").exists() { ("cargo", vec!["test"]) } else if base.join("src-tauri").join("Cargo.toml").exists() { ("cargo", vec!["test", "--manifest-path", "src-tauri/Cargo.toml"]) } else { return Err("No Cargo.toml found in workspace root or src-tauri/.".into()); },
        "python-test" => {
            let supported = ["pytest.ini", "pyproject.toml", "setup.cfg", "tox.ini", "requirements.txt"]
                .iter().any(|name| base.join(name).is_file());
            if !supported { return Err("No Python test configuration or dependency manifest was found.".into()); }
            if cfg!(windows) { ("python", vec!["-m", "pytest"]) } else { ("python3", vec!["-m", "pytest"]) }
        },
        _ => return Err("Allowed checks: npm-build, npm-test, cargo-check, cargo-test, python-test.".into()),
    };
    let output = capture_command(program, &args, &base, Duration::from_secs(180))?;
    let status = if output.timed_out { "timeout" } else if output.status.as_ref().map(ExitStatus::success).unwrap_or(false) { "passed" } else { "failed" };
    Ok(BuildCommandResult { profile, status:status.into(), exit_code:output.status.and_then(|s|s.code()),
        stdout:String::from_utf8_lossy(&output.stdout).into_owned(), stderr: if output.timed_out && output.stderr.is_empty() { "DEVFORGE stopped this check after 180 seconds.".into() } else { String::from_utf8_lossy(&output.stderr).into_owned() }, duration_ms:output.duration_ms })
}

#[tauri::command]
fn native_analyze_codebase(root: String) -> Result<core::CodebaseReport, String> {
    let p = std::path::PathBuf::from(root).canonicalize().map_err(|e| e.to_string())?;
    core::analyze_codebase(&p)
}

#[tauri::command]
fn native_index_workspace(root: String) -> Result<core::WorkspaceIndex, String> {
    let p = std::path::PathBuf::from(root).canonicalize().map_err(|e| e.to_string())?;
    core::index_workspace(&p)
}

#[tauri::command]
fn native_detect_lsp(language: String) -> core::LspServer { core::detect_lsp(&language) }

#[tauri::command]
fn native_language_info(path: String) -> core::LanguageInfo { core::language_for_path(&path) }

#[tauri::command]
fn app_info() -> serde_json::Value {
    serde_json::json!({"name":"DEVFORGE","version":"1.5.1","platform":"windows-native","mode":"desktop","engine":"native-ide-language-github-engine"})
}

#[tauri::command]
fn native_projects_dir(app: tauri::AppHandle) -> Result<String, String> { Ok(projects_base(&app)?.to_string_lossy().to_string()) }

#[tauri::command]
fn create_project(app: tauri::AppHandle, name: String, idea: String) -> Result<NativeProject, String> {
    let base = projects_base(&app)?;
    let slug = safe_slug(&name);
    if slug.is_empty() { return Err("Project name is invalid".into()); }
    let root = base.join(format!("{}-{}", slug, uuid_like()));
    fs::create_dir_all(root.join("src")).map_err(|e| e.to_string())?;
    fs::create_dir_all(root.join("docs")).map_err(|e| e.to_string())?;
    write_if_missing(&root.join("README.md"), &format!("# {}\n\nGenerated by DEVFORGE.\n\n## Idea\n{}\n", name, idea))?;
    write_if_missing(&root.join("src").join("main.js"), "const mode = process.argv[2];\nif (mode === '--test') console.log('DEVFORGE_TEST_PASS');\nelse if (mode === '--build-check') console.log('DEVFORGE_BUILD_PASS');\nelse console.log('DEVFORGE project ready');\n")?;
    write_if_missing(&root.join("package.json"), r#"{
  "name": "devforge-generated-project",
  "private": true,
  "version": "0.1.0",
  "scripts": {
    "build": "node src/main.js --build-check",
    "test": "node src/main.js --test"
  }
}
"#)?;
    write_if_missing(&root.join("docs").join("BUILD_PLAN.md"), &format!("# Build Plan\n\n## Project\n{}\n\n## Idea\n{}\n\nThis workspace was created by the native DEVFORGE runtime.\n", name, idea))?;
    let manifest = serde_json::json!({"name":name,"idea":idea,"createdBy":"DEVFORGE","runtime":"native","engineVersion":"0.6.0"});
    fs::write(root.join("devforge.project.json"), serde_json::to_string_pretty(&manifest).unwrap()).map_err(|e| e.to_string())?;
    Ok(NativeProject{id: root.file_name().unwrap().to_string_lossy().to_string(), name, root: root.to_string_lossy().to_string()})
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    pub root: String,
    pub name: String,
    pub file_count: usize,
    pub languages: Vec<String>,
    pub manifests: Vec<String>,
    pub git_repository: bool,
}

fn canonical_workspace_root(root: &str) -> Result<PathBuf, String> {
    let requested = PathBuf::from(root).canonicalize().map_err(|e| format!("Invalid workspace folder: {e}"))?;
    if !requested.is_dir() { return Err("Workspace path must be a folder.".into()); }
    Ok(requested)
}

fn workspace_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out=Vec::new();
    fn walk(dir:&Path,out:&mut Vec<PathBuf>)->Result<(),String>{
        for e in fs::read_dir(dir).map_err(|e|e.to_string())? {
            let e=e.map_err(|e|e.to_string())?; let p=e.path();
            let n=p.file_name().and_then(|x|x.to_str()).unwrap_or("");
            if ["node_modules","target",".git",".devforge","dist","build",".next","coverage"].contains(&n){continue;}
            if p.is_dir(){walk(&p,out)?} else {out.push(p)}
        }
        Ok(())
    }
    walk(root,&mut out)?; Ok(out)
}

#[tauri::command]
fn open_workspace_folder(path: String) -> Result<WorkspaceInfo, String> {
    let root=canonical_workspace_root(&path)?;
    let files=workspace_files(&root)?;
    let mut langs=std::collections::BTreeSet::new();
    for f in &files { let l=language_for_path(&f.to_string_lossy()); if l!="Plain Text / Unknown" && l!="Plain Text" {langs.insert(l);} }
    let manifests=["package.json","Cargo.toml","pyproject.toml","requirements.txt","go.mod","pom.xml","build.gradle","build.gradle.kts","*.sln","*.csproj","CMakeLists.txt","Makefile","composer.json","Gemfile","pubspec.yaml","mix.exs"];
    let mut found=Vec::new();
    for m in manifests { if m.contains('*') { if files.iter().any(|f| f.file_name().and_then(|x|x.to_str()).map(|n| n.ends_with(&m[1..])).unwrap_or(false)) {found.push(m.into());} } else if root.join(m).exists(){found.push(m.into());} }
    Ok(WorkspaceInfo{root:root.to_string_lossy().to_string(),name:root.file_name().and_then(|x|x.to_str()).unwrap_or("Workspace").into(),file_count:files.len(),languages:langs.into_iter().collect(),manifests:found,git_repository:root.join(".git").exists()})
}

#[tauri::command]
fn list_workspace_files(root: String) -> Result<Vec<String>, String> {
    let base=canonical_workspace_root(&root)?; let mut out=Vec::new();
    for p in workspace_files(&base)? { if let Ok(r)=p.strip_prefix(&base){out.push(r.to_string_lossy().replace('\\', "/"));} }
    out.sort(); out.truncate(5000); Ok(out)
}

#[tauri::command]
fn read_workspace_file(root: String, relative: String) -> Result<FileDocument,String> {
    let base=canonical_workspace_root(&root)?; let rel=PathBuf::from(&relative);
    if rel.is_absolute() || relative.contains("..") || relative.contains(':'){return Err("Invalid workspace-relative path.".into());}
    let file=base.join(&rel); if !file.exists()||!file.is_file(){return Err("File does not exist.".into());}
    let meta=fs::metadata(&file).map_err(|e|e.to_string())?; if meta.len()>8*1024*1024{return Err("File is larger than 8 MB.".into());}
    let bytes=fs::read(&file).map_err(|e|e.to_string())?; if bytes.iter().take(8192).any(|b|*b==0){return Err("Binary files are not opened in the text editor.".into());}
    Ok(FileDocument{path:relative,content:String::from_utf8(bytes).map_err(|_|"File is not UTF-8 text.".to_string())?})
}

#[tauri::command]
fn write_workspace_file(root: String, relative: String, content: String) -> Result<String,String> {
    let base=canonical_workspace_root(&root)?; let rel=PathBuf::from(&relative);
    if rel.is_absolute() || relative.contains("..") || relative.contains(':'){return Err("Invalid workspace-relative path.".into());}
    if content.len()>8*1024*1024{return Err("File is larger than 8 MB.".into());}
    let file=base.join(&rel); if !file.exists()||!file.is_file(){return Err("File does not exist.".into());}
    fs::write(&file,content).map_err(|e|e.to_string())?; Ok(relative)
}

#[tauri::command]
fn list_project_files(root: String) -> Result<Vec<String>, String> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) -> Result<(), String> {
        for e in fs::read_dir(dir).map_err(|e| e.to_string())? {
            let e=e.map_err(|e| e.to_string())?; let p=e.path();
            if ["target","node_modules",".git"].contains(&p.file_name().and_then(|x|x.to_str()).unwrap_or("")) { continue; }
            if p.is_dir() { walk(base,&p,out)?; } else if let Ok(r)=p.strip_prefix(base) { out.push(r.to_string_lossy().replace('\\','/')); }
        }
        Ok(())
    }
    let base=PathBuf::from(root); let mut out=Vec::new(); walk(&base,&base,&mut out)?; out.sort(); Ok(out)
}

#[tauri::command]
fn run_build_command(app: tauri::AppHandle, root: String, profile: String) -> Result<BuildCommandResult, String> {
    let root = safe_project_root(&app, &root)?;
    if !root.join("devforge.project.json").exists() { return Err("Not a DEVFORGE project workspace.".into()); }
    let (program, args, script): (&str, Vec<&str>, &str) = match profile.as_str() {
        "npm-build" => if cfg!(windows) { ("cmd.exe", vec!["/D","/S","/C","npm.cmd run build"], "build") } else { ("npm", vec!["run","build"], "build") },
        "npm-test" => if cfg!(windows) { ("cmd.exe", vec!["/D","/S","/C","npm.cmd test"], "test") } else { ("npm", vec!["test"], "test") },
        _ => return Err("Unsupported build profile. Allowed profiles: npm-build, npm-test.".into()),
    };
    let manifest: serde_json::Value = serde_json::from_slice(&fs::read(root.join("package.json"))
        .map_err(|e| format!("Could not read generated package.json: {e}"))?)
        .map_err(|e| format!("Generated package.json is invalid: {e}"))?;
    if manifest.get("scripts").and_then(|scripts| scripts.get(script)).and_then(serde_json::Value::as_str).is_none() {
        return Err(format!("Generated project does not define the required npm '{script}' script."));
    }
    let output = capture_command(program, &args, &root, Duration::from_secs(180))?;
    let status = if output.timed_out { "timeout" } else if output.status.as_ref().map(ExitStatus::success).unwrap_or(false) { "passed" } else { "failed" };
    Ok(BuildCommandResult { profile, status:status.into(), exit_code:output.status.and_then(|s|s.code()),
        stdout:String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr:if output.timed_out && output.stderr.is_empty() {"DEVFORGE stopped the check after 180 seconds.".into()}else{String::from_utf8_lossy(&output.stderr).into_owned()},
        duration_ms:output.duration_ms })
}


fn safe_project_file(app: &tauri::AppHandle, root: &str, relative: &str) -> Result<PathBuf, String> {
    let project = safe_project_root(app, root)?;
    let rel = PathBuf::from(relative);
    if rel.is_absolute() || relative.contains("..") || relative.contains(':') {
        return Err("Repair patch path must be relative to the project root.".into());
    }
    let file = project.join(rel);
    let parent = file.parent().ok_or_else(|| "Invalid patch path.".to_string())?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let parent_canon = parent.canonicalize().map_err(|e| e.to_string())?;
    if !parent_canon.starts_with(&project) { return Err("Repair patch escaped the project workspace.".into()); }
    Ok(file)
}

fn append_repair_log(project: &Path, event: serde_json::Value) -> Result<String, String> {
    let dir = project.join(".devforge");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("repair-log.json");
    let mut events: Vec<serde_json::Value> = if path.exists() {
        serde_json::from_str(&fs::read_to_string(&path).map_err(|e| e.to_string())?).unwrap_or_default()
    } else { Vec::new() };
    events.push(event);
    fs::write(&path, serde_json::to_string_pretty(&events).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
fn inject_demo_failure(app: tauri::AppHandle, root: String) -> Result<String, String> {
    let project = safe_project_root(&app, &root)?;
    let file = project.join("src").join("main.js");
    let original = fs::read_to_string(&file).map_err(|e| e.to_string())?;
    if original.contains("DEVFORGE_BUILD_PASS") {
        let broken = original.replace("DEVFORGE_BUILD_PASS", "DEVFORGE_BUILD_FAIL");
        fs::write(&file, broken).map_err(|e| e.to_string())?;
        return Ok("Controlled demo failure injected into src/main.js.".into());
    }
    Ok("Demo failure marker is already active.".into())
}

#[tauri::command]
fn run_repair_loop(app: tauri::AppHandle, root: String, max_attempts: Option<u32>) -> Result<RepairResult, String> {
    let project = safe_project_root(&app, &root)?;
    let _attempt_limit = max_attempts.unwrap_or(3).clamp(1, 3);
    let build = run_build_command(app.clone(), root.clone(), "npm-build".into())?;
    let test = if build.status == "passed" { Some(run_build_command(app, root, "npm-test".into())?) } else { None };
    let validated = build.status == "passed" && test.as_ref().map(|result| result.status == "passed").unwrap_or(false);
    let log_path = append_repair_log(&project, serde_json::json!({
        "event":"validation_complete",
        "status":if validated {"passed"} else {"failed"},
        "note":"No repair was applied automatically. Request and approve a repair plan in the AI Codebase Agent."
    }))?;
    Ok(RepairResult {
        status:if validated {"validated"} else {"unresolved"}.into(),
        attempts:1,
        patches_applied:Vec::new(),
        build:Some(build),
        test,
        log_path,
    })
}


#[derive(Debug, Serialize, Deserialize)]
pub struct TerminalResult { pub command: String, pub cwd: String, pub status: String, pub exit_code: Option<i32>, pub stdout: String, pub stderr: String, pub duration_ms: u128 }

#[derive(Debug, Serialize, Deserialize)]
pub struct FileDocument { pub path: String, pub content: String }

#[derive(Debug, Serialize, Deserialize)]
pub struct GitResult { pub status: String, pub stdout: String, pub stderr: String }

fn command_timeout(program: &str, args: &[&str], cwd: &Path, timeout: Duration) -> Result<TerminalResult, String> {
    let output = capture_command(program, args, cwd, timeout)?;
    let status = if output.timed_out { "timeout" } else if output.status.as_ref().map(ExitStatus::success).unwrap_or(false) { "passed" } else { "failed" };
    Ok(TerminalResult {
        command:format!("{} {}",program,args.join(" ")),
        cwd:cwd.to_string_lossy().to_string(),
        status:status.into(),
        exit_code:output.status.and_then(|status|status.code()),
        stdout:String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr:if output.timed_out && output.stderr.is_empty() {"DEVFORGE stopped the process after its safety timeout.".into()}else{String::from_utf8_lossy(&output.stderr).into_owned()},
        duration_ms:output.duration_ms,
    })
}

#[tauri::command]
fn read_project_file(app: tauri::AppHandle, root: String, relative: String) -> Result<FileDocument,String> {
    let project=safe_project_root(&app,&root)?; let file=safe_project_file(&app,&root,&relative)?;
    if !file.exists(){return Err("File does not exist.".into());}
    let meta=fs::metadata(&file).map_err(|e|e.to_string())?; if meta.len()>4*1024*1024{return Err("File is larger than 4 MB.".into());}
    Ok(FileDocument{path:relative,content:fs::read_to_string(file).map_err(|e|format!("Cannot read file: {e}"))?})
}

#[tauri::command]
fn write_project_file(app: tauri::AppHandle, root: String, relative: String, content: String) -> Result<String,String> {
    let _project=safe_project_root(&app,&root)?; if content.len()>4*1024*1024{return Err("File is larger than 4 MB.".into());}
    let file=safe_project_file(&app,&root,&relative)?; fs::write(&file,content).map_err(|e|format!("Cannot write file: {e}"))?; Ok(relative)
}

#[tauri::command]
fn search_project(app: tauri::AppHandle, root: String, query: String) -> Result<Vec<String>,String> {
    let project=safe_project_root(&app,&root)?; if query.trim().is_empty(){return Ok(vec![]);} let q=query.to_lowercase(); let mut hits=Vec::new();
    fn walk(base:&Path,dir:&Path,q:&str,hits:&mut Vec<String>)->Result<(),String>{
        for e in fs::read_dir(dir).map_err(|e|e.to_string())? { let e=e.map_err(|e|e.to_string())?; let p=e.path(); let n=p.file_name().and_then(|x|x.to_str()).unwrap_or(""); if ["node_modules","target",".git",".devforge"].contains(&n){continue;} if p.is_dir(){walk(base,&p,q,hits)?;} else if let Ok(c)=fs::read_to_string(&p){if c.to_lowercase().contains(q){if let Ok(r)=p.strip_prefix(base){hits.push(r.to_string_lossy().replace('\\','/'));}}}} Ok(()) }
    walk(&project,&project,&q,&mut hits)?; hits.sort(); hits.truncate(200); Ok(hits)
}

#[tauri::command]
fn execute_terminal(app: tauri::AppHandle, root: String, command: String, timeout_seconds: Option<u64>) -> Result<TerminalResult,String> {
    let project=safe_project_root(&app,&root)?; if command.trim().is_empty(){return Err("Terminal command cannot be empty.".into());}
    let timeout=Duration::from_secs(timeout_seconds.unwrap_or(120).clamp(1,300));
    if cfg!(windows) { command_timeout("cmd.exe",&["/C",&command],&project,timeout) } else { command_timeout("sh",&["-lc",&command],&project,timeout) }
}

#[tauri::command]
fn open_terminal(app: tauri::AppHandle, root: String, command: Option<String>) -> Result<String,String> {
    let project=safe_project_root(&app,&root)?; let cwd=project.to_string_lossy().to_string();
    if cfg!(windows) { let cmd=command.unwrap_or_else(||"".into()); Command::new("cmd.exe").args(["/K",&cmd]).current_dir(&project).spawn().map_err(|e|e.to_string())?; }
    else { Command::new("x-terminal-emulator").arg("-e").arg(command.unwrap_or_else(||"sh".into())).current_dir(&project).spawn().map_err(|e|e.to_string())?; }
    Ok(cwd)
}


#[tauri::command]
fn execute_workspace_terminal(root: String, command: String, timeout_seconds: Option<u64>) -> Result<TerminalResult,String> {
    let workspace=canonical_workspace_root(&root)?; if command.trim().is_empty(){return Err("Terminal command cannot be empty.".into());}
    let timeout=Duration::from_secs(timeout_seconds.unwrap_or(120).clamp(1,300));
    if cfg!(windows) { command_timeout("cmd.exe", &["/C", &command], &workspace, timeout) } else { command_timeout("sh", &["-lc", &command], &workspace, timeout) }
}

#[tauri::command]
fn open_workspace_terminal(root: String, command: Option<String>) -> Result<String,String> {
    let workspace=canonical_workspace_root(&root); let workspace=workspace?; let cwd=workspace.to_string_lossy().to_string();
    if cfg!(windows) { let cmd=command.unwrap_or_default(); Command::new("cmd.exe").args(["/K",&cmd]).current_dir(&workspace).spawn().map_err(|e|e.to_string())?; }
    else { Command::new("x-terminal-emulator").arg("-e").arg(command.unwrap_or_else(||"sh".into())).current_dir(&workspace).spawn().map_err(|e|e.to_string())?; }
    Ok(cwd)
}

#[tauri::command]
fn workspace_git_status(root: String) -> Result<GitResult,String> {
    let workspace=canonical_workspace_root(&root)?;
    if !workspace.join(".git").exists(){return Ok(GitResult{status:"not-repository".into(),stdout:String::new(),stderr:"No .git directory found in this workspace.".into()});}
    let r=command_timeout("git", &["status","--short","--branch"], &workspace, Duration::from_secs(20))?;
    Ok(GitResult{status:r.status,stdout:r.stdout,stderr:r.stderr})
}

#[tauri::command]
fn workspace_copilot_status(root: String) -> Result<TerminalResult,String> {
    let workspace=canonical_workspace_root(&root)?;
    command_timeout("copilot", &["-sp","Respond with exactly: COPILOT_READY"], &workspace, Duration::from_secs(45))
        .or_else(|_|Ok(TerminalResult{command:"copilot".into(),cwd:workspace.to_string_lossy().to_string(),status:"unavailable".into(),exit_code:None,stdout:String::new(),stderr:"GitHub Copilot CLI is not installed or authenticated.".into(),duration_ms:0}))
}

#[tauri::command]
fn workspace_ask_copilot(root: String, prompt: String) -> Result<TerminalResult,String> {
    let workspace=canonical_workspace_root(&root)?; if prompt.trim().is_empty(){return Err("Copilot prompt cannot be empty.".into());}
    command_timeout("copilot", &["-sp",&prompt], &workspace, Duration::from_secs(180))
        .or_else(|e|Ok(TerminalResult{command:"copilot -sp".into(),cwd:workspace.to_string_lossy().to_string(),status:"unavailable".into(),exit_code:None,stdout:String::new(),stderr:e,duration_ms:0}))
}

#[tauri::command]
fn git_status(app: tauri::AppHandle, root: String) -> Result<GitResult,String> {
    let project=safe_project_root(&app,&root)?; let r=command_timeout(if cfg!(windows){"git"}else{"git"},&["status","--short","--branch"],&project,Duration::from_secs(20))?;
    Ok(GitResult{status:r.status,stdout:r.stdout,stderr:r.stderr})
}

#[tauri::command]
fn copilot_status(app: tauri::AppHandle, root: String) -> Result<TerminalResult,String> {
    let project=safe_project_root(&app,&root)?; let r=command_timeout("copilot",&["-sp","Respond with exactly: COPILOT_READY"],&project,Duration::from_secs(45));
    r.or_else(|_|Ok(TerminalResult{command:"copilot".into(),cwd:project.to_string_lossy().to_string(),status:"unavailable".into(),exit_code:None,stdout:String::new(),stderr:"GitHub Copilot CLI is not installed or authenticated.".into(),duration_ms:0}))
}

#[tauri::command]
fn ask_copilot(app: tauri::AppHandle, root: String, prompt: String) -> Result<TerminalResult,String> {
    let project=safe_project_root(&app,&root)?; if prompt.trim().is_empty(){return Err("Copilot prompt cannot be empty.".into());}
    // Copilot CLI owns GitHub authentication and permission prompts. DEVFORGE invokes its documented non-interactive mode.
    command_timeout("copilot",&["-sp",&prompt],&project,Duration::from_secs(180)).or_else(|e|Ok(TerminalResult{command:"copilot -sp".into(),cwd:project.to_string_lossy().to_string(),status:"unavailable".into(),exit_code:None,stdout:String::new(),stderr:e,duration_ms:0}))
}



fn language_for_path(path: &str) -> String {
    let p = path.to_ascii_lowercase();
    let ext = p.rsplit('.').next().unwrap_or("");
    match ext {
        "js"|"mjs"|"cjs" => "JavaScript".into(), "ts"|"mts"|"cts" => "TypeScript".into(),
        "jsx" => "JavaScript React".into(), "tsx" => "TypeScript React".into(),
        "py" => "Python".into(), "rs" => "Rust".into(), "go" => "Go".into(),
        "java" => "Java".into(), "kt"|"kts" => "Kotlin".into(), "swift" => "Swift".into(),
        "c"|"h" => "C".into(), "cc"|"cpp"|"cxx"|"hpp" => "C++".into(), "cs" => "C#".into(),
        "php" => "PHP".into(), "rb" => "Ruby".into(), "dart" => "Dart".into(),
        "lua" => "Lua".into(), "r" => "R".into(), "scala" => "Scala".into(),
        "ex"|"exs" => "Elixir".into(), "erl"|"hrl" => "Erlang".into(), "hs" => "Haskell".into(),
        "fs"|"fsx" => "F#".into(), "vb" => "Visual Basic".into(), "pl"|"pm" => "Perl".into(),
        "sql" => "SQL".into(), "sh"|"bash" => "Shell".into(), "ps1"|"psm1" => "PowerShell".into(),
        "bat"|"cmd" => "Batch".into(), "asm"|"s" => "Assembly".into(), "sol" => "Solidity".into(),
        "vue" => "Vue".into(), "svelte" => "Svelte".into(), "astro" => "Astro".into(),
        "html"|"htm" => "HTML".into(), "css" => "CSS".into(), "scss" => "SCSS".into(), "sass" => "Sass".into(),
        "less" => "Less".into(), "json"|"jsonc" => "JSON".into(), "xml" => "XML".into(),
        "yaml"|"yml" => "YAML".into(), "toml" => "TOML".into(), "ini"|"cfg"|"conf" => "Config".into(),
        "md"|"markdown" => "Markdown".into(), "txt" => "Plain Text".into(), "graphql"|"gql" => "GraphQL".into(),
        "proto" => "Protocol Buffers".into(), "dockerfile" => "Dockerfile".into(), "tf" => "Terraform".into(),
        "makefile" => "Makefile".into(), "gradle" => "Gradle".into(), "m"|"mm" => "Objective-C".into(),
        _ => if p.ends_with("dockerfile") { "Dockerfile".into() } else { "Plain Text / Unknown".into() }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OpenDocument { pub path: String, pub language: String, pub content: String }

#[tauri::command]
fn open_any_file(path: String) -> Result<OpenDocument, String> {
    let file = PathBuf::from(&path);
    if !file.exists() || !file.is_file() { return Err("Selected path is not a file.".into()); }
    let meta = fs::metadata(&file).map_err(|e| e.to_string())?;
    if meta.len() > 8 * 1024 * 1024 { return Err("Files larger than 8 MB are not opened in the DEVFORGE editor.".into()); }
    let bytes = fs::read(&file).map_err(|e| format!("Cannot read file: {e}"))?;
    if bytes.iter().take(8192).any(|b| *b == 0) { return Err("This appears to be a binary file. DEVFORGE will not edit binary data as text.".into()); }
    let content = String::from_utf8(bytes).map_err(|_| "This file is not valid UTF-8 text.".to_string())?;
    Ok(OpenDocument { path: file.to_string_lossy().to_string(), language: language_for_path(&path), content })
}

#[tauri::command]
fn save_any_file(path: String, content: String) -> Result<String, String> {
    let file = PathBuf::from(&path);
    if !file.exists() || !file.is_file() { return Err("File does not exist.".into()); }
    if content.len() > 8 * 1024 * 1024 { return Err("Files larger than 8 MB are not saved by the DEVFORGE editor.".into()); }
    fs::write(&file, content).map_err(|e| format!("Cannot save file: {e}"))?;
    Ok(file.to_string_lossy().to_string())
}

#[tauri::command]
fn detect_language(path: String) -> String { language_for_path(&path) }

#[tauri::command]
fn copilot_login(app: tauri::AppHandle, root: String) -> Result<String, String> {
    let project = safe_project_root(&app, &root)?;
    if cfg!(windows) {
        Command::new("cmd.exe").args(["/K", "copilot login"]).current_dir(&project).spawn().map_err(|e| format!("Could not open GitHub Copilot login: {e}"))?;
    } else {
        Command::new("sh").args(["-lc", "copilot login"]).current_dir(&project).spawn().map_err(|e| format!("Could not open GitHub Copilot login: {e}"))?;
    }
    Ok("GitHub Copilot login opened. Complete the GitHub authorization in the browser/terminal, then return to DEVFORGE and check the connection.".into())
}


#[derive(Debug, Serialize, Deserialize)]
pub struct Diagnostic { pub severity: String, pub line: usize, pub column: usize, pub message: String, pub source: String }

#[derive(Debug, Serialize, Deserialize)]
pub struct SymbolInfo { pub name: String, pub kind: String, pub line: usize, pub language: String }

#[derive(Debug, Serialize, Deserialize)]
pub struct LanguageCapabilities { pub language: String, pub language_id: String, pub lsp_server: Option<String>, pub server_detected: bool, pub features: Vec<String> }

fn language_id(language: &str) -> String {
    match language { "TypeScript"=>"typescript", "JavaScript"=>"javascript", "Python"=>"python", "Rust"=>"rust", "Go"=>"go", "C#"=>"csharp", "C/C++"=>"cpp", "Java"=>"java", "Kotlin"=>"kotlin", "PHP"=>"php", "Ruby"=>"ruby", "Swift"=>"swift", "Dart"=>"dart", "HTML"=>"html", "CSS"=>"css", "SCSS"=>"scss", "JSON"=>"json", "YAML"=>"yaml", "Markdown"=>"markdown", "SQL"=>"sql", "Shell"=>"shell", "PowerShell"=>"powershell", "Vue"=>"vue", "Svelte"=>"svelte", "Astro"=>"astro", _=>"plaintext" }.into()
}
fn lsp_program(language: &str) -> Option<&'static str> {
    match language { "TypeScript"|"JavaScript"=>Some("typescript-language-server"), "Python"=>Some("pyright-langserver"), "Rust"=>Some("rust-analyzer"), "Go"=>Some("gopls"), "C#"=>Some("csharp-ls"), "C/C++"=>Some("clangd"), "Java"=>Some("jdtls"), "Kotlin"=>Some("kotlin-language-server"), "Ruby"=>Some("solargraph"), "PHP"=>Some("intelephense"), "Dart"=>Some("dart"), _=>None }
}

#[tauri::command]
fn language_capabilities(path: String) -> LanguageCapabilities {
    let language=language_for_path(&path); let program=lsp_program(&language);
    let detected=program.map(|p| Command::new(if cfg!(windows){"where"}else{"which"}).arg(p).output().map(|o|o.status.success()).unwrap_or(false)).unwrap_or(false);
    let features=vec!["syntax-highlighting".into(),"code-folding".into(),"bracket-matching".into(),"find-replace".into(),"document-symbols".into(),"diagnostics".into(),"formatting".into(),"go-to-definition-ready".into(),"completion-ready".into()];
    LanguageCapabilities{language:language.clone(),language_id:language_id(&language),lsp_server:program.map(str::to_string),server_detected:detected,features}
}

fn basic_diagnostics(content: &str, language: &str) -> Vec<Diagnostic> {
    let mut out=Vec::new();
    let pairs=[('(',')'),('{','}'),('[',']')]; let mut stack:Vec<(char,usize,usize)>=Vec::new();
    for (li,line) in content.lines().enumerate(){ for (ci,ch) in line.chars().enumerate(){ if pairs.iter().any(|(a,_)|*a==ch){stack.push((ch,li+1,ci+1));} else if let Some((open,_,_))=stack.last().copied(){ if pairs.iter().find(|(_,b)|*b==ch).map(|(a,_)|*a)==Some(open){stack.pop();} else if pairs.iter().any(|(_,b)|*b==ch){out.push(Diagnostic{severity:"error".into(),line:li+1,column:ci+1,message:format!("Unmatched closing '{}'",ch),source:"DEVFORGE parser".into()});}} }}
    for (ch,l,c) in stack { let expected=match ch{'('=>')','{' =>'}','['=>']',_=>'?'}; out.push(Diagnostic{severity:"error".into(),line:l,column:c,message:format!("Unclosed '{}' (expected '{}')",ch,expected),source:"DEVFORGE parser".into()}); }
    for (li,line) in content.lines().enumerate(){ if language=="JSON" && line.trim_end().ends_with(',') { /* tolerated here; JSON diagnostics remain LSP-ready */ } if line.contains("TODO") { out.push(Diagnostic{severity:"info".into(),line:li+1,column:line.find("TODO").unwrap_or(0)+1,message:"TODO marker".into(),source:"DEVFORGE".into()}); } }
    out
}

#[tauri::command]
fn analyze_document(path: String, content: String) -> Vec<Diagnostic> { basic_diagnostics(&content,&language_for_path(&path)) }

#[tauri::command]
fn document_symbols(path: String, content: String) -> Vec<SymbolInfo> {
    let lang=language_for_path(&path); let mut out=Vec::new();
    for (i,line) in content.lines().enumerate(){ let t=line.trim(); let (kind,name)=if let Some(rest)=t.strip_prefix("function "){("function",rest.split(|c:char|c=='('||c.is_whitespace()).next().unwrap_or(rest))} else if let Some(rest)=t.strip_prefix("const "){("variable",rest.split(|c:char|c=='='||c.is_whitespace()).next().unwrap_or(rest))} else if let Some(rest)=t.strip_prefix("let "){("variable",rest.split(|c:char|c=='='||c.is_whitespace()).next().unwrap_or(rest))} else if let Some(rest)=t.strip_prefix("class "){("class",rest.split_whitespace().next().unwrap_or(rest))} else if let Some(rest)=t.strip_prefix("def "){("function",rest.split(|c:char|c=='('||c.is_whitespace()).next().unwrap_or(rest))} else if let Some(rest)=t.strip_prefix("fn "){("function",rest.split(|c:char|c=='('||c.is_whitespace()).next().unwrap_or(rest))} else {continue}; if !name.is_empty(){out.push(SymbolInfo{name:name.to_string(),kind:kind.to_string(),line:i+1,language:lang.clone()});}}
    out
}

#[tauri::command]
fn format_document(path: String, content: String) -> String {
    let lang=language_for_path(&path); if lang=="JSON" { if let Ok(v)=serde_json::from_str::<serde_json::Value>(&content){ if let Ok(x)=serde_json::to_string_pretty(&v){return x;} } }
    content.lines().map(|l|l.trim_end()).collect::<Vec<_>>().join("\n")
}


#[derive(Debug, Serialize, Deserialize)]
pub struct DebugBreakpoint { pub path: String, pub line: u32, pub enabled: bool }

#[derive(Debug, Serialize, Deserialize)]
pub struct DebugSessionInfo {
    pub id: u64,
    pub status: String,
    pub command: String,
    pub cwd: String,
    pub language: String,
    pub breakpoints: Vec<String>,
    pub output: String,
    pub pid: Option<u32>,
    pub duration_ms: u128,
    pub adapter: Option<String>,
    pub adapter_detected: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DebugAdapterInfo {
    pub language: String,
    pub adapter: Option<String>,
    pub detected: bool,
    pub mode: String,
    pub note: String,
}

fn debug_adapter_for_language(language: &str) -> Option<(&'static str, &'static str)> {
    match language {
        "JavaScript" | "TypeScript" | "JavaScript React" | "TypeScript React" => Some(("js-debug", "js-debug / DAP-compatible adapter")),
        "Python" => Some(("debugpy", "debugpy / DAP-compatible adapter")),
        "Rust" => Some(("codelldb|lldb-dap", "CodeLLDB or LLDB-DAP")),
        "Go" => Some(("dlv", "Delve DAP")),
        "C" | "C++" => Some(("codelldb|lldb-dap|gdb", "CodeLLDB, LLDB-DAP or GDB")),
        "C#" => Some(("netcoredbg", "netcoredbg / DAP-compatible adapter")),
        "Java" => Some(("java-debug", "Java Debug Server")),
        _ => None,
    }
}

fn executable_detected(program: &str) -> bool {
    let probe = if cfg!(windows) { "where" } else { "which" };
    Command::new(probe).arg(program).output().map(|o| o.status.success()).unwrap_or(false)
}

fn adapter_detected(spec: &str) -> bool {
    spec.split('|').any(executable_detected)
}

#[tauri::command]
fn debug_adapter_status(path: String) -> DebugAdapterInfo {
    let language = language_for_path(&path);
    if let Some((spec, note)) = debug_adapter_for_language(&language) {
        DebugAdapterInfo { language, adapter: Some(spec.into()), detected: adapter_detected(spec), mode: "DAP-ready".into(), note: note.into() }
    } else {
        DebugAdapterInfo { language, adapter: None, detected: false, mode: "process-debug-foundation".into(), note: "No standard DEVFORGE debugger adapter is registered for this language yet.".into() }
    }
}

fn spawn_debug_reader<R: std::io::Read + Send + 'static>(reader: R, output: Arc<Mutex<String>>, label: &'static str) {
    std::thread::spawn(move || {
        let mut br = BufReader::new(reader);
        let mut line = String::new();
        loop {
            line.clear();
            match br.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    if let Ok(mut out) = output.lock() {
                        out.push_str(label);
                        out.push_str(&line);
                        if out.len() > 262_144 { let keep = out.len() - 262_144; out.drain(..keep); }
                    }
                }
                Err(_) => break,
            }
        }
    });
}

#[tauri::command]
fn debug_start(
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
    root: String,
    command: String,
    path: String,
    breakpoints: Vec<DebugBreakpoint>,
) -> Result<DebugSessionInfo, String> {
    let project = safe_project_root(&app, &root)?;
    if command.trim().is_empty() { return Err("Debug command cannot be empty.".into()); }
    let language = language_for_path(&path);
    let adapter = debug_adapter_for_language(&language);
    let output = Arc::new(Mutex::new(String::new()));
    let mut child = if cfg!(windows) {
        Command::new("cmd.exe").args(["/C", &command]).current_dir(&project).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
    } else {
        Command::new("sh").args(["-lc", &command]).current_dir(&project).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
    }.map_err(|e| format!("Could not start debug process: {e}"))?;
    if let Some(stdout) = child.stdout.take() { spawn_debug_reader(stdout, output.clone(), "OUT  "); }
    if let Some(stderr) = child.stderr.take() { spawn_debug_reader(stderr, output.clone(), "ERR  "); }
    let id = {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_micros() as u64
    };
    let bp = breakpoints.iter().filter(|b| b.enabled).map(|b| format!("{}:{}", b.path, b.line)).collect::<Vec<_>>();
    let pid = child.id();
    let session = DebugSession { child, command: command.clone(), cwd: project.to_string_lossy().to_string(), language: language.clone(), breakpoints: bp.clone(), output: output.clone(), started_at: Instant::now() };
    state.debug.lock().map_err(|_| "Debugger state is unavailable.".to_string())?.insert(id, session);
    Ok(DebugSessionInfo { id, status: "running".into(), command, cwd: project.to_string_lossy().to_string(), language: language.clone(), breakpoints: bp, output: output.lock().map(|x| x.clone()).unwrap_or_default(), pid: Some(pid), duration_ms: 0, adapter: adapter.map(|x| x.0.into()), adapter_detected: adapter.map(|x| adapter_detected(x.0)).unwrap_or(false) })
}

fn debug_info(state: &AppState, id: u64) -> Result<DebugSessionInfo, String> {
    let mut map = state.debug.lock().map_err(|_| "Debugger state is unavailable.".to_string())?;
    let s = map.get_mut(&id).ok_or_else(|| "Debug session not found.".to_string())?;
    let status = match s.child.try_wait().map_err(|e| e.to_string())? {
        Some(_) => "exited",
        None => "running",
    };
    let adapter = debug_adapter_for_language(&s.language);
    Ok(DebugSessionInfo { id, status: status.into(), command: s.command.clone(), cwd: s.cwd.clone(), language: s.language.clone(), breakpoints: s.breakpoints.clone(), output: s.output.lock().map(|x| x.clone()).unwrap_or_default(), pid: Some(s.child.id()), duration_ms: s.started_at.elapsed().as_millis(), adapter: adapter.map(|x| x.0.into()), adapter_detected: adapter.map(|x| adapter_detected(x.0)).unwrap_or(false) })
}

#[tauri::command]
fn debug_status(state: tauri::State<AppState>, id: u64) -> Result<DebugSessionInfo, String> { debug_info(&state, id) }

#[tauri::command]
fn debug_stop(state: tauri::State<AppState>, id: u64) -> Result<DebugSessionInfo, String> {
    let mut map = state.debug.lock().map_err(|_| "Debugger state is unavailable.".to_string())?;
    let s = map.get_mut(&id).ok_or_else(|| "Debug session not found.".to_string())?;
    let _ = s.child.kill();
    let _ = s.child.wait();
    Ok(DebugSessionInfo { id, status: "stopped".into(), command: s.command.clone(), cwd: s.cwd.clone(), language: s.language.clone(), breakpoints: s.breakpoints.clone(), output: s.output.lock().map(|x| x.clone()).unwrap_or_default(), pid: None, duration_ms: s.started_at.elapsed().as_millis(), adapter: debug_adapter_for_language(&s.language).map(|x| x.0.into()), adapter_detected: debug_adapter_for_language(&s.language).map(|x| adapter_detected(x.0)).unwrap_or(false) })
}

#[tauri::command]
fn debug_set_breakpoints(state: tauri::State<AppState>, id: u64, breakpoints: Vec<DebugBreakpoint>) -> Result<Vec<String>, String> {
    let mut map = state.debug.lock().map_err(|_| "Debugger state is unavailable.".to_string())?;
    let s = map.get_mut(&id).ok_or_else(|| "Debug session not found.".to_string())?;
    s.breakpoints = breakpoints.iter().filter(|b| b.enabled).map(|b| format!("{}:{}", b.path, b.line)).collect();
    Ok(s.breakpoints.clone())
}

#[tauri::command]
fn debug_continue(state: tauri::State<AppState>, id: u64) -> Result<String, String> {
    let _map = state.debug.lock().map_err(|_| "Debugger state is unavailable.".to_string())?;
    Err(format!("Debug session {id} is running. Continue is delegated to the detected DAP adapter in the next debugger integration layer."))
}

#[tauri::command]
fn debug_pause(state: tauri::State<AppState>, id: u64) -> Result<String, String> {
    let _map = state.debug.lock().map_err(|_| "Debugger state is unavailable.".to_string())?;
    Err(format!("Pause for session {id} requires an attached DAP adapter; DEVFORGE will not fake a pause by killing the process."))
}

#[tauri::command]
fn debug_step(state: tauri::State<AppState>, id: u64, kind: String) -> Result<String, String> {
    let _map = state.debug.lock().map_err(|_| "Debugger state is unavailable.".to_string())?;
    if !["over","into","out"].contains(&kind.as_str()) { return Err("Step kind must be over, into, or out.".into()); }
    Err(format!("Step {kind} requires an attached DAP adapter; DEVFORGE has not simulated execution stepping."))
}


#[derive(Debug, Serialize, Deserialize)]
pub struct DapVariable { pub name: String, pub value: String, pub variables_reference: i64 }

#[derive(Debug, Serialize, Deserialize)]
pub struct DapFrame { pub id: i64, pub name: String, pub source: Option<String>, pub line: u32, pub column: u32 }

#[derive(Debug, Serialize, Deserialize)]
pub struct DapStateInfo { pub status: String, pub stopped_reason: Option<String>, pub thread_id: Option<i64>, pub frames: Vec<DapFrame>, pub variables: Vec<DapVariable>, pub output: String }

fn dap_adapter_command(language: &str) -> Result<(String, Vec<String>), String> {
    let env_key = match language {
        "Python" => "DEVFORGE_DAP_PYTHON_COMMAND",
        "Go" => "DEVFORGE_DAP_GO_COMMAND",
        "Rust" | "C" | "C++" => "DEVFORGE_DAP_NATIVE_COMMAND",
        "JavaScript" | "TypeScript" | "JavaScript React" | "TypeScript React" => "DEVFORGE_DAP_JS_COMMAND",
        "C#" => "DEVFORGE_DAP_DOTNET_COMMAND",
        "Java" => "DEVFORGE_DAP_JAVA_COMMAND",
        _ => "DEVFORGE_DAP_COMMAND",
    };
    if let Ok(cmd) = std::env::var(env_key) {
        let parts: Vec<String> = cmd.split_whitespace().map(|x| x.to_string()).collect();
        if !parts.is_empty() { return Ok((parts[0].clone(), parts[1..].to_vec())); }
    }
    match language {
        "Python" => Ok((if cfg!(windows) { "python" } else { "python3" }.into(), vec!["-m".into(), "debugpy.adapter".into()])),
        "Go" => Ok(("dlv".into(), vec!["dap".into()])),
        "Rust" | "C" | "C++" => Ok((if cfg!(windows) { "codelldb.exe" } else { "codelldb" }.into(), vec![])),
        _ => Err(format!("No built-in DAP adapter command for {language}. Set the appropriate DEVFORGE_DAP_*_COMMAND environment variable.")),
    }
}

fn dap_send(child: &mut Child, seq: &mut u64, command: &str, arguments: serde_json::Value) -> Result<serde_json::Value, String> {
    let request_seq = *seq; *seq += 1;
    let payload = serde_json::json!({"seq":request_seq,"type":"request","command":command,"arguments":arguments});
    let body = serde_json::to_vec(&payload).map_err(|e| e.to_string())?;
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    use std::io::{Read, Write};
    child.stdin.as_mut().ok_or("DAP stdin unavailable")?.write_all(header.as_bytes()).map_err(|e| e.to_string())?;
    child.stdin.as_mut().ok_or("DAP stdin unavailable")?.write_all(&body).map_err(|e| e.to_string())?;
    child.stdin.as_mut().ok_or("DAP stdin unavailable")?.flush().map_err(|e| e.to_string())?;
    let stdout = child.stdout.as_mut().ok_or("DAP stdout unavailable")?;
    loop {
        let mut buf = [0u8; 1];
        let mut header_bytes = Vec::new();
        loop {
            stdout.read_exact(&mut buf).map_err(|e| e.to_string())?;
            header_bytes.push(buf[0]);
            if header_bytes.ends_with(b"\r\n\r\n") { break; }
            if header_bytes.len() > 8192 { return Err("DAP header too large".into()); }
        }
        let hs=String::from_utf8_lossy(&header_bytes);
        let len=hs.lines().find_map(|l| l.strip_prefix("Content-Length:").and_then(|v|v.trim().parse::<usize>().ok())).ok_or("DAP response missing Content-Length")?;
        let mut body=vec![0u8;len]; stdout.read_exact(&mut body).map_err(|e| e.to_string())?;
        let msg: serde_json::Value=serde_json::from_slice(&body).map_err(|e| format!("Invalid DAP JSON: {e}"))?;
        if msg.get("type").and_then(|v|v.as_str())==Some("response") && msg.get("request_seq").and_then(|v|v.as_u64())==Some(request_seq) {
            if msg.get("success").and_then(|v|v.as_bool())==Some(false) { return Err(msg.get("message").and_then(|v|v.as_str()).unwrap_or("DAP request failed").into()); }
            return Ok(msg.get("body").cloned().unwrap_or(serde_json::json!({})));
        }
    }
}

fn dap_initialize(child: &mut Child, seq: &mut u64) -> Result<(), String> {
    let _=dap_send(child,seq,"initialize",serde_json::json!({"clientID":"devforge","clientName":"DEVFORGE","adapterID":"devforge","linesStartAt1":true,"columnsStartAt1":true,"pathFormat":"path","supportsRunInTerminalRequest":false}))?;
    Ok(())
}

#[tauri::command]
fn dap_start(state: tauri::State<AppState>, root: String, path: String, language: String, program: String, args: Vec<String>, breakpoints: Vec<u32>) -> Result<u64, String> {
    let (adapter,args0)=dap_adapter_command(&language)?;
    let mut child=Command::new(&adapter).args(args0).current_dir(&root).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e|format!("Could not start DAP adapter {adapter}: {e}"))?;
    let mut seq=1u64;
    dap_initialize(&mut child,&mut seq)?;
    let source_path=Path::new(&root).join(&path);
    let _=dap_send(&mut child,&mut seq,"setBreakpoints",serde_json::json!({"source":{"path":source_path.to_string_lossy()},"breakpoints":breakpoints.iter().map(|l|serde_json::json!({"line":l})).collect::<Vec<_>>() }))?;
    let _=dap_send(&mut child,&mut seq,"launch",serde_json::json!({"program":Path::new(&root).join(&program).to_string_lossy(),"args":args,"cwd":root,"stopOnEntry":false}))?;
    let id=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e|e.to_string())?.as_micros() as u64;
    let _=dap_send(&mut child,&mut seq,"configurationDone",serde_json::json!({}))?;
    state.dap.lock().map_err(|_|"DAP state unavailable".to_string())?.insert(id,DapSession{child,seq,initialized:true});
    Ok(id)
}

fn with_dap<F,R>(state:&AppState,id:u64,f:F)->Result<R,String> where F:FnOnce(&mut DapSession)->Result<R,String>{
    let mut map=state.dap.lock().map_err(|_|"DAP state unavailable".to_string())?;
    let s=map.get_mut(&id).ok_or_else(||"DAP session not found".to_string())?;
    f(s)
}

#[tauri::command]
fn dap_continue(state: tauri::State<AppState>, id:u64, thread_id:i64)->Result<String,String>{with_dap(&state,id,|s|{let _=dap_send(&mut s.child,&mut s.seq,"continue",serde_json::json!({"threadId":thread_id}))?;Ok("continued".into())})}
#[tauri::command]
fn dap_pause(state: tauri::State<AppState>, id:u64, thread_id:i64)->Result<String,String>{with_dap(&state,id,|s|{let _=dap_send(&mut s.child,&mut s.seq,"pause",serde_json::json!({"threadId":thread_id}))?;Ok("pause requested".into())})}
#[tauri::command]
fn dap_step(state: tauri::State<AppState>, id:u64, thread_id:i64, kind:String)->Result<String,String>{with_dap(&state,id,|s|{let cmd=match kind.as_str(){"over"=>"next","into"=>"stepIn","out"=>"stepOut",_=>return Err("step kind must be over, into, or out".into())};let _=dap_send(&mut s.child,&mut s.seq,cmd,serde_json::json!({"threadId":thread_id}))?;Ok(format!("step {kind} requested"))})}
#[tauri::command]
fn dap_threads(state: tauri::State<AppState>, id:u64)->Result<serde_json::Value,String>{with_dap(&state,id,|s|dap_send(&mut s.child,&mut s.seq,"threads",serde_json::json!({})))}
#[tauri::command]
fn dap_stack(state: tauri::State<AppState>, id:u64, thread_id:i64)->Result<serde_json::Value,String>{with_dap(&state,id,|s|dap_send(&mut s.child,&mut s.seq,"stackTrace",serde_json::json!({"threadId":thread_id,"levels":50})))}
#[tauri::command]
fn dap_scopes(state: tauri::State<AppState>, id:u64, frame_id:i64)->Result<serde_json::Value,String>{with_dap(&state,id,|s|dap_send(&mut s.child,&mut s.seq,"scopes",serde_json::json!({"frameId":frame_id})))}
#[tauri::command]
fn dap_variables(state: tauri::State<AppState>, id:u64, variables_reference:i64)->Result<serde_json::Value,String>{with_dap(&state,id,|s|dap_send(&mut s.child,&mut s.seq,"variables",serde_json::json!({"variablesReference":variables_reference})))}
#[tauri::command]
fn dap_evaluate(state: tauri::State<AppState>, id:u64, expression:String, frame_id:Option<i64>)->Result<serde_json::Value,String>{with_dap(&state,id,|s|dap_send(&mut s.child,&mut s.seq,"evaluate",serde_json::json!({"expression":expression,"frameId":frame_id,"context":"repl"})))}
#[tauri::command]
fn dap_stop(state: tauri::State<AppState>, id:u64)->Result<String,String>{let mut map=state.dap.lock().map_err(|_|"DAP state unavailable".to_string())?;let mut s=map.remove(&id).ok_or_else(||"DAP session not found".to_string())?;let _=s.child.kill();let _=s.child.wait();Ok("stopped".into())}

fn uuid_like() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos(); format!("{:x}", n)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![workspace_agent_context, workspace_authorize_agent_patches, workspace_apply_agent_patches, workspace_restore_agent_backups, workspace_run_check, native_analyze_codebase, app_info, native_projects_dir, create_project, list_project_files, run_build_command, inject_demo_failure, run_repair_loop, read_project_file, write_project_file, search_project, execute_terminal, open_terminal, execute_workspace_terminal, open_workspace_terminal, workspace_git_status, workspace_copilot_status, workspace_ask_copilot, git_status, copilot_status, ask_copilot, open_any_file, save_any_file, detect_language, open_workspace_folder, list_workspace_files, read_workspace_file, write_workspace_file, copilot_login, language_capabilities, analyze_document, document_symbols, format_document, debug_adapter_status, debug_start, debug_status, debug_stop, debug_set_breakpoints, debug_continue, debug_pause, debug_step, dap_start, dap_continue, dap_pause, dap_step, dap_threads, dap_stack, dap_scopes, dap_variables, dap_evaluate, dap_stop,
        native_index_workspace,
        native_detect_lsp,
        native_language_info])
        .run(tauri::generate_context!())
        .expect("error while running DEVFORGE");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_workspace(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("devforge-{label}-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn rejects_traversal_and_protected_agent_paths() {
        for path in ["../outside.txt", "src/../../outside.txt", ".devforge/backups/file", "node_modules/pkg/index.js"] {
            assert!(validate_agent_relative_path(path).is_err(), "accepted {path}");
        }
        assert!(validate_agent_relative_path("src/main.ts").is_ok());
    }

    #[test]
    fn rejects_malformed_binary_non_unique_and_stale_patches() {
        let mut patch = AgentPatch { path: "src/main.ts".into(), find: "target".into(), replace: "changed".into(), reason: "test".into(), expected_hash: agent_text_hash("target") };
        assert_eq!(apply_agent_patch_text("target", &patch).unwrap(), "changed");
        patch.find.clear();
        assert!(apply_agent_patch_text("target", &patch).is_err());
        patch.find = "target".into();
        patch.expected_hash = agent_text_hash("changed");
        assert!(apply_agent_patch_text("target", &patch).unwrap_err().contains("changed after planning"));
        patch.expected_hash = agent_text_hash("target target");
        assert!(apply_agent_patch_text("target target", &patch).unwrap_err().contains("found 2"));
        patch.expected_hash = agent_text_hash("a\0b");
        assert!(apply_agent_patch_text("a\0b", &patch).unwrap_err().contains("binary"));
    }

    #[test]
    fn writes_require_a_matching_unexpired_one_time_approval() {
        let state = AppState::default();
        let root = temp_workspace("approval");
        let patch = AgentPatch { path: "src/main.ts".into(), find: "before".into(), replace: "after".into(), reason: "test".into(), expected_hash: agent_text_hash("before") };
        assert!(consume_agent_approval(&state, "missing", &root, std::slice::from_ref(&patch)).is_err());
        state.agent_approvals.lock().unwrap().insert("once".into(), AgentApproval {
            root: root.clone(), plan_digest: agent_plan_digest(std::slice::from_ref(&patch)).unwrap(), expires_at: Instant::now() + Duration::from_secs(30),
        });
        assert!(consume_agent_approval(&state, "once", &root, std::slice::from_ref(&patch)).is_ok());
        assert!(consume_agent_approval(&state, "once", &root, std::slice::from_ref(&patch)).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restores_backups_only_when_the_changed_file_is_unmodified() {
        let root = temp_workspace("restore");
        let relative = Path::new("src").join("main.ts");
        fs::create_dir_all(root.join(relative.parent().unwrap())).unwrap();
        fs::write(root.join(&relative), "after").unwrap();
        let backup = Path::new(".devforge").join("backups").join("agent-test").join(&relative);
        fs::create_dir_all(root.join(backup.parent().unwrap())).unwrap();
        fs::write(root.join(&backup), "before").unwrap();
        let backup_path = backup.to_string_lossy().replace('\\', "/");
        let record = AgentBackup { path: backup_path, expected_hash: agent_text_hash("after") };
        assert_eq!(restore_agent_backups_at(&root, std::slice::from_ref(&record)).unwrap(), vec!["src/main.ts"]);
        assert_eq!(fs::read_to_string(root.join(&relative)).unwrap(), "before");
        fs::write(root.join(&relative), "later edit").unwrap();
        assert!(restore_agent_backups_at(&root, std::slice::from_ref(&record)).unwrap_err().contains("changed after"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn timeout_stops_the_command_and_captures_its_result() {
        if std::env::var_os("DEVFORGE_TIMEOUT_TEST_CHILD").is_some() {
            thread::sleep(Duration::from_secs(5));
            return;
        }
        let executable = std::env::current_exe().unwrap();
        let path = std::env::temp_dir();
        let args = ["--exact", "tests::timeout_stops_the_command_and_captures_its_result", "--nocapture"];
        std::env::set_var("DEVFORGE_TIMEOUT_TEST_CHILD", "1");
        let result = capture_command(executable.to_str().unwrap(), &args, path, Duration::from_millis(150)).unwrap();
        std::env::remove_var("DEVFORGE_TIMEOUT_TEST_CHILD");
        assert!(result.timed_out);
        assert!(result.status.is_none());
        assert!(result.duration_ms < 2_000);
    }
}

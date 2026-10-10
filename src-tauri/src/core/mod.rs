//! DEVFORGE native core.
//! The UI shell may evolve independently; file parsing, workspace indexing,
//! language detection and tool orchestration live here so they are not tied to JS.

use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fs, path::{Path, PathBuf}, process::{Command, Stdio}};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageInfo { pub id: String, pub extensions: Vec<String>, pub server: Option<String> }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceIndex { pub root: String, pub files: usize, pub languages: Vec<LanguageInfo>, pub manifests: Vec<String>, pub git: bool }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspServer { pub language: String, pub command: String, pub available: bool }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionManifest { pub id: String, pub name: String, pub version: String, pub runtime: String, pub permissions: Vec<String> }

pub fn language_for_path(path: &str) -> LanguageInfo {
    let p = path.to_ascii_lowercase();
    let table = [
        ("rust", &[".rs"][..], Some("rust-analyzer")), ("typescript", &[".ts", ".tsx"], Some("typescript-language-server")),
        ("javascript", &[".js", ".jsx", ".mjs", ".cjs"], Some("typescript-language-server")), ("python", &[".py", ".pyi"], Some("pyright-langserver")),
        ("go", &[".go"], Some("gopls")), ("cpp", &[".cpp", ".cc", ".cxx", ".hpp", ".h"], Some("clangd")),
        ("csharp", &[".cs"], Some("csharp-ls")), ("java", &[".java"], Some("jdtls")), ("kotlin", &[".kt", ".kts"], Some("kotlin-language-server")),
        ("php", &[".php"], Some("intelephense")), ("ruby", &[".rb"], Some("ruby-lsp")), ("swift", &[".swift"], Some("sourcekit-lsp")),
        ("dart", &[".dart"], Some("dart")), ("lua", &[".lua"], Some("lua-language-server")), ("sql", &[".sql"], None),
        ("html", &[".html", ".htm"], None), ("css", &[".css", ".scss", ".sass", ".less"], None),
        ("json", &[".json", ".jsonc"], None), ("yaml", &[".yml", ".yaml"], None), ("markdown", &[".md", ".markdown"], None),
        ("shell", &[".sh", ".bash"], None), ("powershell", &[".ps1", ".psm1"], None), ("zig", &[".zig"], Some("zls")),
    ];
    for (id, exts, server) in table { if exts.iter().any(|e| p.ends_with(e)) { return LanguageInfo{id:id.into(),extensions:exts.iter().map(|s|s.to_string()).collect(),server:server.map(str::to_string)}; } }
    LanguageInfo{id:"text".into(),extensions:vec![],server:None}
}

fn walk(root: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(root)? { let p=entry?.path(); let name=p.file_name().and_then(|s|s.to_str()).unwrap_or(""); if [".git","node_modules","target","dist","build","coverage",".devforge"].contains(&name) { continue; } if fs::symlink_metadata(&p)?.file_type().is_symlink(){continue;} if p.is_dir(){walk(&p,out)?} else {out.push(p)} }
    Ok(())
}

pub fn index_workspace(root: &Path) -> Result<WorkspaceIndex, String> {
    let mut files=Vec::new(); walk(root,&mut files).map_err(|e|e.to_string())?;
    let mut ids=BTreeSet::new(); let mut manifests=Vec::new();
    for f in &files { let info=language_for_path(&f.to_string_lossy()); ids.insert(info.id.clone()); if let Some(n)=f.file_name().and_then(|s|s.to_str()) { if ["package.json","Cargo.toml","pyproject.toml","go.mod","pom.xml","build.gradle","CMakeLists.txt","requirements.txt","*.sln"].contains(&n) { manifests.push(n.to_string()); } } }
    let languages=ids.into_iter().map(|id| language_for_path(&format!("file.{}", id))).collect();
    Ok(WorkspaceIndex{root:root.to_string_lossy().into(),files:files.len(),languages,manifests,git:root.join(".git").exists()})
}

pub fn detect_lsp(language: &str) -> LspServer {
    let command=match language { "rust"=>"rust-analyzer", "typescript"|"javascript"=>"typescript-language-server", "python"=>"pyright-langserver", "go"=>"gopls", "cpp"=>"clangd", "csharp"=>"csharp-ls", "java"=>"jdtls", "kotlin"=>"kotlin-language-server", "zig"=>"zls", _=>"" };
    if command.is_empty(){return LspServer{language:language.into(),command:String::new(),available:false};}
    let available=Command::new(if cfg!(windows){"where"}else{"which"}).arg(command).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s|s.success()).unwrap_or(false);
    LspServer{language:language.into(),command:command.into(),available}
}

#[cfg(feature="wasm-extensions")]
pub mod wasm {
    use wasmtime::{Engine, Module, Store};
    pub fn validate_module(bytes: &[u8]) -> Result<(), String> { let engine=Engine::default(); Module::new(&engine, bytes).map(|_|()).map_err(|e|e.to_string()) }
    pub fn create_store() -> Store<()> { Store::new(&Engine::default(), ()) }
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodebaseReport { pub root:String, pub files:Vec<FileNode>, pub symbols:Vec<CodeSymbol>, pub imports:Vec<ImportEdge>, pub dependencies:Vec<DependencyInfo>, pub entry_points:Vec<String>, pub git_history:Vec<GitCommit>, pub architecture:ArchitectureSummary }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct FileNode { pub path:String, pub language:String, pub bytes:u64 }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct CodeSymbol { pub path:String, pub name:String, pub kind:String, pub line:usize }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct ImportEdge { pub path:String, pub target:String }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct DependencyInfo { pub manager:String, pub name:String, pub version:String }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct GitCommit { pub hash:String, pub subject:String }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct ArchitectureSummary { pub frameworks:Vec<String>, pub languages:Vec<String>, pub manifests:Vec<String>, pub git:bool }

fn extract_symbols_imports(path:&Path, root:&Path, syms:&mut Vec<CodeSymbol>, imports:&mut Vec<ImportEdge>) {
    let bytes=match fs::read(path){Ok(b)=>b,Err(_)=>return};
    if bytes.len()>2_000_000 || bytes.iter().take(8192).any(|b|*b==0){return;}
    let text=match String::from_utf8(bytes){Ok(s)=>s,Err(_)=>return};
    let rel=path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\','/');
    for (i,line) in text.lines().enumerate(){
        let t=line.trim();
        for (prefix,kind) in [("fn ","function"),("function ","function"),("class ","class"),("interface ","interface"),("struct ","struct"),("enum ","enum"),("def ","function"),("func ","function")].iter(){
            if let Some(rest)=t.strip_prefix(prefix){let name=rest.split(|c:char| c=='('||c=='{'||c==':'||c.is_whitespace()).next().unwrap_or("");if !name.is_empty(){syms.push(CodeSymbol{path:rel.clone(),name:name.into(),kind:(*kind).into(),line:i+1});}}
        }
        if t.starts_with("import ")||t.starts_with("from ")||t.starts_with("use ")||t.starts_with("require(")||t.starts_with("#include ")||t.starts_with("using "){
            let target=t.split_whitespace().skip(1).collect::<Vec<_>>().join(" ").trim_matches(';').trim_matches('"').trim_matches('<').trim_matches('>').to_string();
            if !target.is_empty(){imports.push(ImportEdge{path:rel.clone(),target});}
        }
    }
}

pub fn analyze_codebase(root:&Path)->Result<CodebaseReport,String>{
    let mut paths=Vec::new(); walk(root,&mut paths).map_err(|e|e.to_string())?;
    let mut files=Vec::new(); let mut syms=Vec::new(); let mut imports=Vec::new(); let mut langs=BTreeSet::new(); let mut manifests=Vec::new();
    for f in &paths { let info=language_for_path(&f.to_string_lossy()); langs.insert(info.id.clone()); let bytes=fs::metadata(f).map(|m|m.len()).unwrap_or(0); let rel=f.strip_prefix(root).unwrap_or(f).to_string_lossy().replace('\\','/'); files.push(FileNode{path:rel,language:info.id.clone(),bytes}); extract_symbols_imports(f,root,&mut syms,&mut imports); if let Some(n)=f.file_name().and_then(|x|x.to_str()){ if ["package.json","Cargo.toml","pyproject.toml","requirements.txt","go.mod","pom.xml","build.gradle","build.gradle.kts","CMakeLists.txt","composer.json","Gemfile","pubspec.yaml","mix.exs"].iter().any(|m|*m==n)||n.ends_with(".sln"){manifests.push(n.into());}} }
    let mut deps=Vec::new();
    let package=root.join("package.json"); if let Ok(text)=fs::read_to_string(package){if let Ok(v)=serde_json::from_str::<serde_json::Value>(&text){for section in ["dependencies","devDependencies"]{if let Some(o)=v.get(section).and_then(|x|x.as_object()){for(k,val) in o{deps.push(DependencyInfo{manager:"npm".into(),name:k.clone(),version:val.as_str().unwrap_or("*").into()});}}}}}
    for (name,manager) in [("requirements.txt","pip"),("go.mod","go"),("Cargo.toml","cargo")] {let p=root.join(name);if let Ok(text)=fs::read_to_string(p){for line in text.lines(){let t=line.trim();if t.is_empty()||t.starts_with('#')||t.starts_with('['){continue} if manager=="pip"{let n=t.split(['=','>','<']).next().unwrap_or(t).trim();deps.push(DependencyInfo{manager:manager.into(),name:n.into(),version:t.into()});} else if let Some((k,v))=t.split_once('='){deps.push(DependencyInfo{manager:manager.into(),name:k.trim().into(),version:v.trim().trim_matches('"').into()});}}}}
    let entries=files.iter().filter(|f|["main.rs","main.go","main.py","app.py","index.js","index.ts","index.tsx","main.js","main.ts","program.cs","program.java","main.cpp","main.c"].iter().any(|x|f.path.to_ascii_lowercase().ends_with(x))).map(|f|f.path.clone()).collect::<Vec<_>>();
    let mut history=Vec::new(); if root.join(".git").exists(){if let Ok(out)=Command::new("git").args(["-C",root.to_string_lossy().as_ref(),"log","-n","20","--pretty=format:%h%x09%s"]).output(){for line in String::from_utf8_lossy(&out.stdout).lines(){let mut p=line.splitn(2,'\t');if let(Some(h),Some(sub))=(p.next(),p.next()){history.push(GitCommit{hash:h.into(),subject:sub.into()});}}}}
    let frameworks=deps.iter().filter_map(|d|{let n=d.name.to_ascii_lowercase();if ["react","vite","next","express","tauri","electron","vue","svelte","django","flask","fastapi","tokio","actix-web","axum"].contains(&n.as_str()){Some(d.name.clone())}else{None}}).collect();
    Ok(CodebaseReport{root:root.to_string_lossy().into(),files,symbols:syms.into_iter().take(10000).collect(),imports:imports.into_iter().take(20000).collect(),dependencies:deps.into_iter().take(5000).collect(),entry_points:entries,git_history:history,architecture:ArchitectureSummary{frameworks,languages:langs.into_iter().collect(),manifests,git:root.join(".git").exists()}})
}

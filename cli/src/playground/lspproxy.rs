//! Real completions for the Go and Rust run editors: the machine's own
//! language servers (gopls, rust-analyzer), spoken to over LSP stdio against
//! a persistent workspace holding the generated SDK plus the snippet as the
//! main file. The UI posts the snippet and a position to `/api/complete`; a
//! missing language server degrades to the editor's static fallback.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Mutex, OnceLock};

use tono_backend::codegen::TargetKind;

use super::{json_error, json_response, Response};

pub fn available() -> Vec<&'static str> {
    let mut out = Vec::new();
    if probe("gopls") {
        out.push("go");
    }
    if probe("rust-analyzer") {
        out.push("rust");
    }
    out
}

fn probe(program: &str) -> bool {
    Command::new(program)
        .arg("version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// A minimal LSP client over child stdio. Requests are sequential (one
/// completion at a time, under the workspace lock), so no id demultiplexing
/// is needed: read until our id answers, acknowledging server-to-client
/// requests along the way so the server never stalls waiting on us.
struct Lsp {
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    child: Child,
    next_id: i64,
}

impl Lsp {
    fn spawn(program: &str, root: &Path) -> Result<Lsp, String> {
        let mut child = Command::new(program)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("cannot spawn {program}: {e}"))?;
        let stdin = child.stdin.take().expect("piped stdin");
        let reader = BufReader::new(child.stdout.take().expect("piped stdout"));
        Ok(Lsp {
            stdin,
            reader,
            child,
            next_id: 1,
        })
    }

    fn send(&mut self, message: &serde_json::Value) -> Result<(), String> {
        let body = message.to_string();
        write!(self.stdin, "Content-Length: {}\r\n\r\n{body}", body.len())
            .and_then(|_| self.stdin.flush())
            .map_err(|e| format!("lsp write: {e}"))
    }

    fn read(&mut self) -> Result<serde_json::Value, String> {
        let mut length: usize = 0;
        loop {
            let mut line = String::new();
            self.reader
                .read_line(&mut line)
                .map_err(|e| format!("lsp read: {e}"))?;
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some(value) = line.strip_prefix("Content-Length:") {
                length = value.trim().parse().unwrap_or(0);
            }
        }
        let mut body = vec![0u8; length];
        self.reader
            .read_exact(&mut body)
            .map_err(|e| format!("lsp read: {e}"))?;
        serde_json::from_slice(&body).map_err(|e| format!("lsp parse: {e}"))
    }

    fn notify(&mut self, method: &str, params: serde_json::Value) -> Result<(), String> {
        self.send(&serde_json::json!({ "jsonrpc": "2.0", "method": method, "params": params }))
    }

    fn request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params
        }))?;
        loop {
            let message = self.read()?;
            let is_request = message.get("method").is_some() && message.get("id").is_some();
            if is_request {
                // Server-to-client request (registerCapability, configuration):
                // an empty answer keeps the conversation moving.
                let reply_id = message
                    .get("id")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let result = if message.get("method").and_then(|m| m.as_str())
                    == Some("workspace/configuration")
                {
                    // One entry per requested item, all defaults.
                    let n = message
                        .pointer("/params/items")
                        .and_then(|i| i.as_array())
                        .map(|a| a.len())
                        .unwrap_or(0);
                    serde_json::Value::Array(vec![serde_json::Value::Null; n])
                } else {
                    serde_json::Value::Null
                };
                self.send(&serde_json::json!({
                    "jsonrpc": "2.0", "id": reply_id, "result": result
                }))?;
                continue;
            }
            if message.get("id").and_then(|i| i.as_i64()) == Some(id) {
                if let Some(error) = message.get("error") {
                    return Err(format!("lsp error: {error}"));
                }
                return Ok(message
                    .get("result")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null));
            }
            // Notifications (diagnostics, progress) are not ours to handle.
        }
    }
}

impl Drop for Lsp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Workspace {
    lsp: Lsp,
    main_uri: String,
    version: i64,
    sdk_hash: u64,
}

static WORKSPACES: OnceLock<Mutex<HashMap<String, Workspace>>> = OnceLock::new();

fn hash_of(source: &str, module: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    module.hash(&mut hasher);
    hasher.finish()
}

fn uri_of(path: &Path) -> String {
    format!("file://{}", path.display())
}

#[derive(serde::Deserialize)]
struct CompleteRequest {
    target: String,
    source: String,
    #[serde(default)]
    module: Option<String>,
    snippet: String,
    line: u32,
    character: u32,
}

pub fn handle(body: &str) -> Response {
    let request: CompleteRequest = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => return json_error(400, &format!("bad request: {e}")),
    };
    match complete(&request) {
        Ok(items) => json_response(200, serde_json::json!({ "items": items })),
        Err(message) => json_error(422, &message),
    }
}

fn complete(request: &CompleteRequest) -> Result<Vec<serde_json::Value>, String> {
    let (kind, server, main_rel, lang_id) = match request.target.as_str() {
        "go" => (TargetKind::Go, "gopls", "main.go", "go"),
        "rust" => (TargetKind::Rust, "rust-analyzer", "src/main.rs", "rust"),
        other => return Err(format!("no language server for {other}")),
    };
    if !probe(server) {
        return Err(format!("{server} is not installed"));
    }
    let module = request.module.clone().unwrap_or_default();
    let sdk_hash = hash_of(&request.source, &module);
    let map = WORKSPACES.get_or_init(Mutex::default);
    let mut map = map.lock().map_err(|_| "workspace lock poisoned")?;

    if let Some(ws) = map.get(&request.target) {
        // A dead or stale server is dropped and respawned below.
        if ws.sdk_hash != sdk_hash {
            map.remove(&request.target);
        }
    }

    if !map.contains_key(&request.target) {
        let root = std::env::temp_dir().join(format!(
            "tono-playground-lsp-{}-{}",
            std::process::id(),
            request.target
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let scratch = root.join("_src");
        std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
        let files =
            super::run::generate_for(&scratch, &request.source, request.module.as_deref(), kind)?;
        let project = root.join("project");
        match kind {
            TargetKind::Go => super::run::scaffold_go(&files, &project, &request.snippet),
            TargetKind::Rust => super::run::scaffold_rust(&files, &project, &request.snippet),
            TargetKind::TypeScript => unreachable!("rejected above"),
        }
        .map_err(|e| format!("scaffold: {e}"))?;

        let mut lsp = Lsp::spawn(server, &project)?;
        let root_uri = uri_of(&project);
        lsp.request(
            "initialize",
            serde_json::json!({
                "processId": std::process::id(),
                "rootUri": root_uri,
                "workspaceFolders": [{ "uri": root_uri, "name": "tono-playground" }],
                "capabilities": {
                    "textDocument": {
                        "completion": { "completionItem": { "documentationFormat": ["plaintext", "markdown"] } }
                    },
                    "workspace": { "configuration": true }
                }
            }),
        )?;
        lsp.notify("initialized", serde_json::json!({}))?;
        let main_uri = uri_of(&project.join(main_rel));
        lsp.notify(
            "textDocument/didOpen",
            serde_json::json!({
                "textDocument": {
                    "uri": main_uri,
                    "languageId": lang_id,
                    "version": 1,
                    "text": request.snippet,
                }
            }),
        )?;
        map.insert(
            request.target.clone(),
            Workspace {
                lsp,
                main_uri,
                version: 1,
                sdk_hash,
            },
        );
    }

    let ws = map.get_mut(&request.target).expect("inserted above");
    ws.version += 1;
    let result = (|| {
        ws.lsp.notify(
            "textDocument/didChange",
            serde_json::json!({
                "textDocument": { "uri": ws.main_uri, "version": ws.version },
                "contentChanges": [{ "text": request.snippet }],
            }),
        )?;
        ws.lsp.request(
            "textDocument/completion",
            serde_json::json!({
                "textDocument": { "uri": ws.main_uri },
                "position": { "line": request.line, "character": request.character },
            }),
        )
    })();
    let result = match result {
        Ok(r) => r,
        Err(e) => {
            // The server died mid-conversation; a fresh one gets built on the
            // next request.
            map.remove(&request.target);
            return Err(e);
        }
    };

    let items = result
        .get("items")
        .and_then(|i| i.as_array())
        .cloned()
        .or_else(|| result.as_array().cloned())
        .unwrap_or_default();
    Ok(items
        .into_iter()
        .take(120)
        .map(|item| {
            let doc = item
                .pointer("/documentation/value")
                .or_else(|| item.get("documentation"))
                .and_then(|d| d.as_str())
                .unwrap_or("");
            serde_json::json!({
                "label": item.get("label").and_then(|l| l.as_str()).unwrap_or(""),
                "kind": item.get("kind").and_then(|k| k.as_u64()).unwrap_or(1),
                "detail": item.get("detail").and_then(|d| d.as_str()).unwrap_or(""),
                "documentation": doc,
                "insertText": item.get("insertText").and_then(|t| t.as_str()).unwrap_or(""),
            })
        })
        .collect())
}

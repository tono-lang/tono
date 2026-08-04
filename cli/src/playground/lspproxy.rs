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

/// The language server for a target: the well-known binary, overridable with
/// an environment variable for custom installs (a gopls outside PATH, a
/// wrapper script).
fn server_program(default: &str, env_key: &str) -> String {
    std::env::var(env_key).unwrap_or_else(|_| default.to_string())
}

fn complete(request: &CompleteRequest) -> Result<Vec<serde_json::Value>, String> {
    let (kind, server, main_rel, lang_id) = match request.target.as_str() {
        "go" => (
            TargetKind::Go,
            server_program("gopls", "TONO_GOPLS"),
            "main.go",
            "go",
        ),
        "rust" => (
            TargetKind::Rust,
            server_program("rust-analyzer", "TONO_RUST_ANALYZER"),
            "src/main.rs",
            "rust",
        ),
        other => return Err(format!("no language server for {other}")),
    };
    if !probe(&server) {
        return Err(format!("{server} is not installed"));
    }
    let module = request.module.clone().unwrap_or_default();
    let sdk_hash = hash_of(&request.source, &module);
    let generate = |scratch: &Path| {
        super::run::generate_for(scratch, &request.source, request.module.as_deref(), kind)
    };
    query(
        &request.target,
        &server,
        kind,
        main_rel,
        lang_id,
        generate,
        &request.snippet,
        request.line,
        request.character,
        sdk_hash,
    )
}

/// The workspace machinery behind [`complete`], parameterized over the server
/// program and the SDK generation so it is testable without a real language
/// server or the OCaml frontend on PATH.
#[allow(clippy::too_many_arguments)]
fn query(
    target_key: &str,
    server: &str,
    kind: TargetKind,
    main_rel: &str,
    lang_id: &str,
    generate: impl FnOnce(&Path) -> Result<Vec<tono_backend::codegen::GeneratedFile>, String>,
    snippet: &str,
    line: u32,
    character: u32,
    sdk_hash: u64,
) -> Result<Vec<serde_json::Value>, String> {
    let map = WORKSPACES.get_or_init(Mutex::default);
    let mut map = map.lock().map_err(|_| "workspace lock poisoned")?;

    if let Some(ws) = map.get(target_key) {
        // A dead or stale server is dropped and respawned below.
        if ws.sdk_hash != sdk_hash {
            map.remove(target_key);
        }
    }

    if !map.contains_key(target_key) {
        let root = std::env::temp_dir().join(format!(
            "tono-playground-lsp-{}-{}",
            std::process::id(),
            target_key
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let scratch = root.join("_src");
        std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
        let files = generate(&scratch)?;
        let project = root.join("project");
        match kind {
            TargetKind::Go => super::run::scaffold_go(&files, &project, snippet),
            TargetKind::Rust => super::run::scaffold_rust(&files, &project, snippet),
            TargetKind::TypeScript => return Err("no language server for typescript".into()),
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
                    "text": snippet,
                }
            }),
        )?;
        map.insert(
            target_key.to_string(),
            Workspace {
                lsp,
                main_uri,
                version: 1,
                sdk_hash,
            },
        );
    }

    let ws = map.get_mut(target_key).expect("inserted above");
    ws.version += 1;
    let result = (|| {
        ws.lsp.notify(
            "textDocument/didChange",
            serde_json::json!({
                "textDocument": { "uri": ws.main_uri, "version": ws.version },
                "contentChanges": [{ "text": snippet }],
            }),
        )?;
        ws.lsp.request(
            "textDocument/completion",
            serde_json::json!({
                "textDocument": { "uri": ws.main_uri },
                "position": { "line": line, "character": character },
            }),
        )
    })();
    let result = match result {
        Ok(r) => r,
        Err(e) => {
            // The server died mid-conversation; a fresh one gets built on the
            // next request.
            map.remove(target_key);
            return Err(e);
        }
    };

    Ok(completion_items(&result))
}

/// Flatten an LSP completion answer (a bare array or a CompletionList) into
/// the wire items the editor consumes, documentation in either LSP shape.
fn completion_items(result: &serde_json::Value) -> Vec<serde_json::Value> {
    let items = result
        .get("items")
        .and_then(|i| i.as_array())
        .cloned()
        .or_else(|| result.as_array().cloned())
        .unwrap_or_default();
    items
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
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A python stand-in speaking just enough LSP: it answers every request
    /// (initialize, completion) and ignores notifications.
    #[cfg(unix)]
    fn write_fake_server(name: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let fake = std::env::temp_dir().join(name);
        // concat! keeps the python indentation exact; a \ continuation would
        // strip it and break the script.
        let script = concat!(
            "#!/usr/bin/env python3\n",
            "import sys, json\n",
            "if len(sys.argv) > 1:\n",
            "    print('fake 0.0')\n",
            "    sys.exit(0)\n",
            "def read():\n",
            "    n = 0\n",
            "    while True:\n",
            "        line = sys.stdin.buffer.readline().decode()\n",
            "        if not line or line.strip() == '':\n",
            "            break\n",
            "        if line.lower().startswith('content-length:'):\n",
            "            n = int(line.split(':')[1])\n",
            "    return json.loads(sys.stdin.buffer.read(n)) if n else None\n",
            "def send(m):\n",
            "    b = json.dumps(m).encode()\n",
            "    h = ('Content-Length: %d' % len(b)).encode()\n",
            "    sys.stdout.buffer.write(h + b'\\r\\n\\r\\n' + b)\n",
            "    sys.stdout.buffer.flush()\n",
            "while True:\n",
            "    m = read()\n",
            "    if m is None:\n",
            "        break\n",
            "    if 'id' in m:\n",
            "        r = {'capabilities': {}}\n",
            "        if m.get('method') == 'textDocument/completion':\n",
            "            r = {'items': [{'label': 'FakeDone', 'kind': 3}]}\n",
            "        send({'jsonrpc': '2.0', 'id': m['id'], 'result': r})\n",
        );
        std::fs::write(&fake, script).expect("fake server");
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        fake
    }

    #[test]
    fn probing_a_missing_binary_is_false_not_an_error() {
        assert!(!probe("definitely-not-a-language-server-xyz"));
        // Whatever this machine has installed, probing must only answer.
        let _ = available();
    }

    #[test]
    fn workspace_hashes_track_source_and_module() {
        assert_eq!(hash_of("a", "m"), hash_of("a", "m"));
        assert_ne!(hash_of("a", "m"), hash_of("b", "m"));
        assert_ne!(hash_of("a", "m"), hash_of("a", "n"));
    }

    #[test]
    fn uris_are_file_scheme() {
        assert!(uri_of(std::path::Path::new("/tmp/x")).starts_with("file:///tmp/x"));
    }

    #[test]
    fn handle_rejects_malformed_and_serverless_targets() {
        assert_eq!(handle("{oops").status_code(), tiny_http::StatusCode(400));
        let unknown = handle(
            &serde_json::json!({
                "target": "cobol", "source": "", "snippet": "", "line": 0, "character": 0
            })
            .to_string(),
        );
        assert_eq!(unknown.status_code(), tiny_http::StatusCode(422));
    }

    #[test]
    fn completion_answers_flatten_in_both_lsp_shapes() {
        // A bare array and a CompletionList carry the same items; markup and
        // plain documentation both surface.
        let bare = serde_json::json!([
            { "label": "GetAccount", "kind": 2, "detail": "func()",
              "documentation": { "kind": "markdown", "value": "docs" } },
            { "label": "x", "documentation": "plain" }
        ]);
        let items = completion_items(&bare);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["label"], "GetAccount");
        assert_eq!(items[0]["documentation"], "docs");
        assert_eq!(items[1]["kind"], 1);
        assert_eq!(items[1]["documentation"], "plain");
        let list = serde_json::json!({ "isIncomplete": false, "items": [{ "label": "y" }] });
        assert_eq!(completion_items(&list).len(), 1);
        assert_eq!(completion_items(&serde_json::Value::Null).len(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn the_whole_workspace_flow_runs_against_a_scripted_server() {
        // A python stand-in speaking just enough LSP: it answers every request
        // (initialize, completion) and ignores notifications, so one test
        // walks scaffold, spawn, handshake, didOpen, didChange, completion,
        // the cached-workspace path, and the stale-hash rebuild.
        let fake = write_fake_server(&format!("tono-fake-ls-{}.py", std::process::id()));
        let program = fake.to_string_lossy().into_owned();

        let ask = |hash: u64| {
            query(
                "fake-go",
                &program,
                TargetKind::Go,
                "main.go",
                "go",
                |_| Ok(Vec::new()),
                "package main\nfunc main() {}\n",
                0,
                0,
                hash,
            )
        };
        let first = ask(1).expect("cold workspace answers");
        assert_eq!(first[0]["label"], "FakeDone");
        // Same hash reuses the running server; a new hash rebuilds it.
        assert_eq!(ask(1).expect("warm workspace answers").len(), 1);
        assert_eq!(ask(2).expect("rebuilt workspace answers").len(), 1);
        let _ = std::fs::remove_file(&fake);
    }

    #[cfg(unix)]
    #[test]
    fn complete_answers_end_to_end_with_overridden_binaries() {
        use std::os::unix::fs::PermissionsExt;
        let _env = crate::playground::playground_env_guard();
        // A stand-in frontend and a scripted language server, both wired
        // through their environment overrides, drive complete() end to end
        // without any real toolchain installed.
        let pid = std::process::id();
        let frontend = std::env::temp_dir().join(format!("tono-fake-frontend-c-{pid}"));
        let ir = serde_json::json!({
            "tono_ir_version": tono_backend::ir::TONO_IR_VERSION,
            "modules": [{
                "name": "playground",
                "shapes": [{
                    "id": "playground#note",
                    "kind": "structure",
                    "params": [],
                    "members": [{
                        "constraints": [],
                        "name": "id",
                        "required": true,
                        "target": { "prim": "string" },
                        "traits": []
                    }],
                    "traits": [{ "id": "pub", "value": null }]
                }],
                "operations": [],
                "extensions": []
            }]
        });
        std::fs::write(&frontend, format!("#!/bin/sh\ncat <<'EOF'\n{ir}\nEOF\n"))
            .expect("frontend");
        std::fs::set_permissions(&frontend, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let server = write_fake_server(&format!("tono-fake-ls-c-{pid}.py"));
        std::env::set_var("TONO_FRONTEND", &frontend);
        std::env::set_var("TONO_GOPLS", &server);

        let request: CompleteRequest = serde_json::from_value(serde_json::json!({
            "target": "go",
            "source": "pub struct note { id: string }",
            "module": "playground",
            "snippet": "package main\nfunc main() {}\n",
            "line": 0,
            "character": 0
        }))
        .expect("request");
        let items = complete(&request);
        std::env::remove_var("TONO_FRONTEND");
        std::env::remove_var("TONO_GOPLS");
        let _ = std::fs::remove_file(&frontend);
        let _ = std::fs::remove_file(&server);
        let items = items.expect("completes");
        assert_eq!(items[0]["label"], "FakeDone");
    }

    #[cfg(unix)]
    #[test]
    fn lsp_framing_round_trips_through_a_cat_echo() {
        // cat echoes our framed request back: the client first sees a message
        // carrying both id and method (a server-to-client request), answers
        // it, then reads its own echoed answer, whose id matches, and
        // resolves. One spawn exercises framing, dispatch, and the reply path.
        let dir = std::env::temp_dir();
        let mut lsp = Lsp::spawn("cat", &dir).expect("spawns");
        let result = lsp
            .request("test/echo", serde_json::json!({ "x": 1 }))
            .expect("round-trips");
        assert_eq!(result, serde_json::Value::Null);
    }
}

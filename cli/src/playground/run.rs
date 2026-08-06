//! `/api/run`: execute a user snippet against the SDK generated from the
//! posted source, using the machine's real toolchains. HTTP never leaves the
//! machine: a mock server bound to loopback answers from the posted route
//! table, and the snippet's environment points the SDK at it (any `$MOCK` in
//! an env value becomes the mock server's URL).
//!
//! The scaffolds extend the compile-check ones: the same generated-source
//! layout plus a main written from the snippet. Every target's generated SDK
//! carries its own transport, so the scaffold needs no runtime dependency.

use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tono_backend::codegen::check::GO_SCAFFOLD_MODULE;
use tono_backend::codegen::GeneratedFile;
use tono_backend::codegen::{casing_for, check_layout, generate_target, CodegenConfig, TargetKind};
use tono_backend::ir::decode_model;

use super::{json_error, json_response, Response};
use crate::frontend::{Frontend, FrontendError};

/// How long a run may take end to end. Generous because the first Rust run
/// cold-compiles the runtime's dependency tree.
const RUN_TIMEOUT: Duration = Duration::from_secs(300);

/// Which targets this machine can execute, probed by the tool each scaffold
/// invokes. TypeScript needs a runner that resolves bare specifiers and runs
/// TS sources directly (bun, or tsx); without one, the browser still runs it.
pub fn available_targets() -> Vec<&'static str> {
    let mut targets = Vec::new();
    if probe("cargo") {
        targets.push("rust");
    }
    if probe("go") {
        targets.push("go");
    }
    if ts_runner().is_some() {
        targets.push("ts");
    }
    targets
}

fn probe(tool: &str) -> bool {
    std::process::Command::new(tool)
        .arg("version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// The TypeScript runner installed here, if any. Both resolve node_modules
/// packages whose entry is TypeScript source and honor tsconfig paths, which
/// is exactly what the scaffold relies on.
fn ts_runner() -> Option<&'static str> {
    // Unlike go and cargo, both runners only answer --version.
    ["bun", "tsx"].into_iter().find(|runner| {
        std::process::Command::new(runner)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

pub fn handle(body: &str) -> Response {
    match serde_json::from_str::<RunRequest>(body) {
        Ok(request) => match execute(&request) {
            Ok(lines) => json_response(200, serde_json::json!({ "lines": lines })),
            Err(message) => json_error(422, &message),
        },
        Err(e) => json_error(400, &format!("bad request: {e}")),
    }
}

#[derive(serde::Deserialize)]
struct RunRequest {
    source: String,
    target: String,
    snippet: String,
    #[serde(default)]
    module: Option<String>,
    #[serde(default)]
    mocks: Mocks,
}

/// The module name becomes the scratch filename the frontend derives the
/// module from, so it is folded to a bare snake_case identifier here; anything
/// else (separators, dots) could escape the scratch directory.
fn sanitize_module(raw: Option<&str>) -> String {
    let folded: String = raw
        .unwrap_or("")
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = folded
        .trim_start_matches(|c: char| c.is_ascii_digit() || c == '_')
        .trim_end_matches('_');
    if trimmed.is_empty() {
        "playground".to_string()
    } else {
        trimmed.to_string()
    }
}

#[derive(serde::Deserialize, Default)]
struct Mocks {
    #[serde(default)]
    routes: std::collections::BTreeMap<String, MockRoute>,
    #[serde(default)]
    env: std::collections::BTreeMap<String, String>,
}

#[derive(serde::Deserialize, Clone)]
struct MockRoute {
    #[serde(default)]
    status: Option<u16>,
    #[serde(default)]
    body: Option<serde_json::Value>,
}

#[derive(serde::Serialize, Clone, Debug)]
pub(crate) struct Line {
    kind: &'static str,
    text: String,
}

fn line(kind: &'static str, text: impl Into<String>) -> Line {
    Line {
        kind,
        text: text.into(),
    }
}

static RUN_COUNTER: AtomicU64 = AtomicU64::new(0);

fn execute(request: &RunRequest) -> Result<Vec<Line>, String> {
    let kind = match request.target.as_str() {
        "rust" => TargetKind::Rust,
        "go" => TargetKind::Go,
        "ts" | "typescript" => TargetKind::TypeScript,
        other => return Err(format!("run does not handle target {other} on the server")),
    };

    // The same compile path the preview uses: the OCaml frontend as a
    // subprocess, IR on stdout.
    let scratch = std::env::temp_dir().join(format!(
        "tono-playground-run-{}-{}",
        std::process::id(),
        RUN_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
    let result = execute_in(&scratch, request, kind);
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

/// Compile the posted source with the native frontend and generate one
/// target's SDK, exactly as a run does. Shared with the LSP workspaces, which
/// need the same files on disk for gopls and rust-analyzer to read.
pub(crate) fn generate_for(
    scratch: &std::path::Path,
    source: &str,
    module: Option<&str>,
    kind: TargetKind,
) -> Result<Vec<GeneratedFile>, String> {
    let source_path = scratch.join(format!("{}.tono", sanitize_module(module)));
    std::fs::write(&source_path, source).map_err(|e| e.to_string())?;
    let frontend = Frontend::from_env();
    let ir = match frontend.compile(&source_path) {
        Ok(ir) => ir,
        Err(FrontendError::Diagnostics(report)) => return Err(report),
        Err(FrontendError::Unavailable { program }) => {
            return Err(format!("frontend unavailable: {program}"))
        }
    };
    let model = decode_model(&ir)?;
    let config = CodegenConfig {
        go_module: Some(GO_SCAFFOLD_MODULE.to_string()),
        ..CodegenConfig::default()
    };
    check_layout(&model, &[kind], &config)?;
    generate_target(&model, kind, &config, &casing_for(kind))
}

fn execute_in(
    scratch: &std::path::Path,
    request: &RunRequest,
    kind: TargetKind,
) -> Result<Vec<Line>, String> {
    let files = generate_for(scratch, &request.source, request.module.as_deref(), kind)?;

    let project = scratch.join(kind.dir());
    match kind {
        TargetKind::Rust => scaffold_rust(&files, &project, &request.snippet),
        TargetKind::Go => scaffold_go(&files, &project, &request.snippet),
        TargetKind::TypeScript => scaffold_typescript(&files, &project, &request.snippet),
    }
    .map_err(|e| format!("scaffold: {e}"))?;

    let mock = MockServer::start(request.mocks.routes.clone())?;
    let mut env: Vec<(String, String)> = request
        .mocks
        .env
        .iter()
        .map(|(k, v)| (k.clone(), v.replace("$MOCK", &mock.url)))
        .collect();
    // Go module hygiene: a parent go.work or a proxy hiccup must not leak into
    // the throwaway project.
    if kind == TargetKind::Go {
        env.push(("GOWORK".into(), "off".into()));
    }

    let (program, args): (&str, Vec<&str>) = match kind {
        TargetKind::Rust => ("cargo", vec!["run", "--quiet"]),
        TargetKind::Go => ("go", vec!["run", "."]),
        TargetKind::TypeScript => {
            let runner = ts_runner().ok_or("no TypeScript runner (bun or tsx) on PATH")?;
            (runner, vec!["main.ts"])
        }
    };
    let mut lines = run_child(program, &args, &project, &env)?;
    let requests = mock.stop();
    // Interleaving is not reconstructable across processes; requests are
    // appended after the program's own output, each marked as such.
    lines.extend(requests);
    Ok(lines)
}

/// Write each generated file under `root`, stripping the leading
/// target-directory segment, exactly like the compile-check scaffold.
fn write_sources(
    files: &[GeneratedFile],
    kind: TargetKind,
    root: &std::path::Path,
) -> std::io::Result<()> {
    for file in files.iter().filter(|f| f.target == kind) {
        let relative = file.path.strip_prefix(kind.dir()).unwrap_or(&file.path);
        let dest = root.join(relative);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dest, &file.text)?;
    }
    Ok(())
}

/// A binary crate over the generated SDK: the generated `lib.rs` is kept (it
/// carries the module tree and re-exports), and the snippet becomes
/// `src/main.rs`.
pub(crate) fn scaffold_rust(
    files: &[GeneratedFile],
    root: &std::path::Path,
    snippet: &str,
) -> std::io::Result<()> {
    let src = root.join("src");
    // An SDK with no files for this target still scaffolds: the directories
    // exist regardless of what write_sources created.
    std::fs::create_dir_all(&src)?;
    write_sources(files, TargetKind::Rust, &src)?;
    std::fs::write(src.join("main.rs"), snippet)?;
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\n\
         name = \"tono_run\"\n\
         version = \"0.0.0\"\n\
         edition = \"2021\"\n\
         [dependencies]\n\
         serde = { version = \"1\", features = [\"derive\"] }\n\
         serde_json = \"1\"\n\
         tokio = { version = \"1\", features = [\"rt-multi-thread\", \"macros\", \"time\"] }\n\
         reqwest = { version = \"0.12\", default-features = false, features = [\"rustls-tls\"] }\n\
         [workspace]\n",
    )
}

/// A Go module over the generated packages: `main.go` is the snippet.
pub(crate) fn scaffold_go(
    files: &[GeneratedFile],
    root: &std::path::Path,
    snippet: &str,
) -> std::io::Result<()> {
    std::fs::create_dir_all(root)?;
    write_sources(files, TargetKind::Go, root)?;
    std::fs::write(root.join("main.go"), snippet)?;
    std::fs::write(
        root.join("go.mod"),
        format!("module {GO_SCAFFOLD_MODULE}\n\ngo 1.21\n"),
    )
}

/// The generated TS sources plus the snippet, with a tsconfig mapping "sdk" to
/// the module barrel, so the same snippet works here and in the browser
/// bundler.
fn scaffold_typescript(
    files: &[GeneratedFile],
    root: &std::path::Path,
    snippet: &str,
) -> std::io::Result<()> {
    write_sources(files, TargetKind::TypeScript, root)?;
    std::fs::write(root.join("main.ts"), snippet)?;
    let barrel = files
        .iter()
        .filter(|f| f.target == TargetKind::TypeScript)
        .filter_map(|f| f.path.strip_prefix(TargetKind::TypeScript.dir()).ok())
        .find(|p| p.ends_with("index.ts"))
        .map(|p| format!("./{}", p.display()))
        .unwrap_or_else(|| "./index.ts".to_string());
    std::fs::write(
        root.join("tsconfig.json"),
        format!(
            "{{\n  \"compilerOptions\": {{\n    \"baseUrl\": \".\",\n    \"paths\": {{ \"sdk\": [\"{barrel}\"] }}\n  }}\n}}\n"
        ),
    )
}

fn run_child(
    program: &str,
    args: &[&str],
    cwd: &std::path::Path,
    env: &[(String, String)],
) -> Result<Vec<Line>, String> {
    let mut command = std::process::Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for (key, value) in env {
        command.env(key, value);
    }
    let mut child = command
        .spawn()
        .map_err(|_| format!("toolchain missing: {program}"))?;

    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let out = std::thread::spawn(move || read_all(stdout));
    let err = std::thread::spawn(move || read_all(stderr));

    let deadline = Instant::now() + RUN_TIMEOUT;
    let status = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => break Some(status),
            None if Instant::now() > deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };

    let stdout = out.join().unwrap_or_default();
    let stderr = err.join().unwrap_or_default();
    let mut lines = Vec::new();
    for text in stdout.lines() {
        lines.push(line("log", text));
    }
    // On success, stderr is toolchain chatter (compiler warnings, progress);
    // only a failed run reports it as errors.
    let stderr_kind = match status {
        Some(s) if s.success() => "log",
        _ => "error",
    };
    for text in stderr.lines() {
        lines.push(line(stderr_kind, text));
    }
    match status {
        None => lines.push(line(
            "error",
            format!("timed out after {}s", RUN_TIMEOUT.as_secs()),
        )),
        Some(status) if !status.success() => {
            lines.push(line("error", format!("exit: {status}")));
        }
        Some(_) => {}
    }
    Ok(lines)
}

fn read_all(mut reader: impl Read) -> String {
    let mut buffer = String::new();
    let _ = reader.read_to_string(&mut buffer);
    buffer
}

/// Whether a declared path template matches a concrete request path: same
/// segment count, and a {label} segment matches any non-empty segment. This is
/// what lets a mock be declared per operation, in the operation's own terms.
fn template_matches(template: &str, path: &str) -> bool {
    let t: Vec<&str> = template.split('/').collect();
    let p: Vec<&str> = path.split('/').collect();
    t.len() == p.len()
        && t.iter().zip(&p).all(|(seg, part)| {
            if seg.starts_with('{') && seg.ends_with('}') {
                !part.is_empty()
            } else {
                seg == part
            }
        })
}

/// A loopback HTTP server answering from the posted route table, so the SDK's
/// real transport has something to talk to. Every request is recorded.
struct MockServer {
    url: String,
    seen: Arc<Mutex<Vec<Line>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: std::thread::JoinHandle<()>,
}

impl MockServer {
    fn start(routes: std::collections::BTreeMap<String, MockRoute>) -> Result<MockServer, String> {
        let server =
            tiny_http::Server::http("127.0.0.1:0").map_err(|e| format!("mock server: {e}"))?;
        let addr = server.server_addr();
        let url = format!("http://{addr}");
        let seen: Arc<Mutex<Vec<Line>>> = Arc::default();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread = {
            let seen = Arc::clone(&seen);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let request = match server.recv_timeout(Duration::from_millis(100)) {
                        Ok(Some(request)) => request,
                        Ok(None) => continue,
                        Err(_) => break,
                    };
                    let method = request.method().to_string().to_uppercase();
                    let path = request.url().split('?').next().unwrap_or("/").to_string();
                    let hit = routes
                        .get(&format!("{method} {path}"))
                        .cloned()
                        .or_else(|| {
                            routes.iter().find_map(|(key, route)| {
                                let (m, template) = key.split_once(' ')?;
                                (m == method && template_matches(template, &path))
                                    .then(|| route.clone())
                            })
                        });
                    seen.lock().expect("mock log").push(line(
                        "request",
                        format!(
                            "{method} {path}{}",
                            if hit.is_some() { "" } else { " (no mock, 404)" }
                        ),
                    ));
                    let (status, body) = match hit {
                        Some(route) => (
                            route.status.unwrap_or(200),
                            route.body.unwrap_or(serde_json::json!({})),
                        ),
                        None => (
                            404,
                            serde_json::json!({ "error": format!("no mock for {method} {path}") }),
                        ),
                    };
                    let response = tiny_http::Response::from_data(body.to_string().into_bytes())
                        .with_status_code(status)
                        .with_header(
                            tiny_http::Header::from_bytes(
                                &b"Content-Type"[..],
                                &b"application/json"[..],
                            )
                            .expect("static header"),
                        );
                    let _ = request.respond(response);
                }
            })
        };
        Ok(MockServer {
            url,
            seen,
            stop,
            thread,
        })
    }

    fn stop(self) -> Vec<Line> {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.thread.join();
        self.seen.lock().expect("mock log").clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, text: &str) -> GeneratedFile {
        GeneratedFile {
            target: TargetKind::Go,
            path: std::path::PathBuf::from(path),
            text: text.to_string(),
        }
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tono-run-test-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn available_targets_only_probe_never_panic() {
        // Contents depend on the machine's toolchains; the call must simply
        // answer without touching anything else.
        let _ = available_targets();
    }

    #[test]
    fn module_names_fold_to_bare_snake_case_identifiers() {
        // The name becomes a scratch filename, so separators and dots must
        // never survive: anything else could escape the directory.
        assert_eq!(sanitize_module(Some("github_api")), "github_api");
        assert_eq!(sanitize_module(Some("My SDK!")), "my_sdk");
        assert_eq!(sanitize_module(Some("../../etc/passwd")), "etc_passwd");
        assert_eq!(sanitize_module(Some("123")), "playground");
        assert_eq!(sanitize_module(None), "playground");
    }

    #[test]
    fn path_templates_match_per_segment() {
        assert!(template_matches("/users/{username}", "/users/gandarfh"));
        assert!(!template_matches("/users/{username}", "/users/"));
        assert!(!template_matches("/users/{username}", "/users/a/b"));
        assert!(template_matches("/account", "/account"));
        assert!(!template_matches("/account", "/other"));
    }

    #[test]
    fn go_scaffold_lays_out_module_and_main() {
        let root = scratch("go-scaffold");
        let files = vec![file("go/playground/types.go", "package playground\n")];
        scaffold_go(&files, &root, "package main\nfunc main() {}\n").expect("scaffold");
        let go_mod = std::fs::read_to_string(root.join("go.mod")).expect("go.mod");
        assert!(go_mod.contains(GO_SCAFFOLD_MODULE));
        assert!(root.join("main.go").is_file());
        assert!(root.join("playground/types.go").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rust_scaffold_lays_out_a_crate_with_no_runtime_dependency() {
        let root = scratch("rust-scaffold");
        let files = vec![GeneratedFile {
            target: TargetKind::Rust,
            path: std::path::PathBuf::from("rust/lib.rs"),
            text: "pub mod nothing {}\n".into(),
        }];
        scaffold_rust(&files, &root, "fn main() {}\n").expect("scaffold");
        let manifest = std::fs::read_to_string(root.join("Cargo.toml")).expect("Cargo.toml");
        assert!(!manifest.contains("sdk-http-runtime-rs"));
        assert!(root.join("src/main.rs").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn run_child_reports_stdout_stderr_and_exit() {
        let dir = scratch("run-child");
        let lines = run_child("sh", &["-c", "echo out; echo err 1>&2"], &dir, &[]).expect("runs");
        assert!(lines.iter().any(|l| l.kind == "log" && l.text == "out"));
        // A successful run reports stderr as chatter, not as errors.
        assert!(lines.iter().any(|l| l.kind == "log" && l.text == "err"));
        let lines = run_child("sh", &["-c", "echo boom 1>&2; exit 3"], &dir, &[]).expect("runs");
        assert!(lines.iter().any(|l| l.kind == "error" && l.text == "boom"));
        assert!(lines
            .iter()
            .any(|l| l.kind == "error" && l.text.starts_with("exit:")));
        let missing = run_child("definitely-not-a-tool-xyz", &[], &dir, &[]);
        assert!(missing.unwrap_err().contains("toolchain missing"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_child_passes_the_environment() {
        let dir = scratch("run-env");
        let env = vec![("TONO_TEST_VALUE".to_string(), "42".to_string())];
        let lines =
            run_child("sh", &["-c", "printf %s \"$TONO_TEST_VALUE\""], &dir, &env).expect("runs");
        assert!(lines.iter().any(|l| l.text == "42"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mock_server_answers_routes_and_records_requests() {
        use std::io::{Read, Write};
        let mut routes = std::collections::BTreeMap::new();
        routes.insert(
            "GET /users/{username}".to_string(),
            MockRoute {
                status: Some(201),
                body: Some(serde_json::json!({ "ok": true })),
            },
        );
        // A bare route falls back to 200 with an empty object.
        routes.insert(
            "GET /bare".to_string(),
            MockRoute {
                status: None,
                body: None,
            },
        );
        let server = MockServer::start(routes).expect("starts");
        let addr = server.url.trim_start_matches("http://").to_string();

        let ask = |path: &str| -> String {
            let mut stream = std::net::TcpStream::connect(&addr).expect("connect");
            write!(
                stream,
                "GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"
            )
            .expect("write");
            let mut out = String::new();
            stream.read_to_string(&mut out).expect("read");
            out
        };
        // The template answers a concrete path; anything else is a 404.
        let hit = ask("/users/gandarfh");
        assert!(hit.starts_with("HTTP/1.1 201"), "{hit}");
        assert!(hit.contains("\"ok\":true"));
        let miss = ask("/nope");
        assert!(miss.starts_with("HTTP/1.1 404"), "{miss}");
        let bare = ask("/bare");
        assert!(bare.starts_with("HTTP/1.1 200"), "{bare}");

        let seen = server.stop();
        assert!(seen.iter().any(|l| l.text.contains("/users/gandarfh")));
        assert!(seen
            .iter()
            .any(|l| l.text.contains("/nope") && l.text.contains("no mock")));
    }

    #[cfg(unix)]
    #[test]
    fn execute_runs_or_reports_the_missing_toolchain() {
        use std::os::unix::fs::PermissionsExt;
        // A stand-in frontend answers `compile` with a fixed valid model, so
        // the whole path runs even where the OCaml build is absent: codegen,
        // scaffold, mock server, child run. With go installed the empty main
        // runs; without it the verdict is the missing toolchain, never a
        // panic.
        let _env = crate::playground::playground_env_guard();
        let fake = std::env::temp_dir().join(format!("tono-fake-frontend-{}", std::process::id()));
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
        std::fs::write(&fake, format!("#!/bin/sh\ncat <<'EOF'\n{ir}\nEOF\n")).expect("fake");
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        std::env::set_var("TONO_FRONTEND", &fake);

        let request: RunRequest = serde_json::from_value(serde_json::json!({
            "source": "pub struct note { id: string }",
            "target": "go",
            "module": "playground",
            "snippet": "package main\n\nfunc main() {}\n",
            "mocks": { "env": { "UNUSED": "$MOCK" } }
        }))
        .expect("request");
        let outcome = execute(&request);
        std::env::remove_var("TONO_FRONTEND");
        let _ = std::fs::remove_file(&fake);
        match outcome {
            Ok(lines) => assert!(!lines.iter().any(|l| l.kind == "error"), "{lines:?}"),
            Err(message) => assert!(message.contains("toolchain missing"), "{message}"),
        }
    }

    #[test]
    fn handle_rejects_malformed_and_unknown_requests() {
        let bad = handle("{not json");
        assert_eq!(bad.status_code(), tiny_http::StatusCode(400));
        let unknown = handle(
            &serde_json::json!({
                "source": "", "target": "cobol", "snippet": ""
            })
            .to_string(),
        );
        assert_eq!(unknown.status_code(), tiny_http::StatusCode(422));
    }
}

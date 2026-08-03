//! `/api/run`: execute a user snippet against the SDK generated from the
//! posted source, using the machine's real toolchains. HTTP never leaves the
//! machine: a mock server bound to loopback answers from the posted route
//! table, and the snippet's environment points the SDK at it (any `$MOCK` in
//! an env value becomes the mock server's URL).
//!
//! The scaffolds extend the compile-check ones: the same generated-source
//! layout plus a main written from the snippet, and the HTTP runtimes the
//! generated SDK imports, unpacked from copies embedded in this binary so an
//! installed CLI works without a checkout.

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

#[derive(rust_embed::Embed)]
#[folder = "../runtimes/http-go"]
#[include = "*.go"]
#[include = "go.mod"]
#[exclude = "*_test.go"]
struct GoRuntime;

#[derive(rust_embed::Embed)]
#[folder = "../runtimes/http-rust"]
#[include = "src/*.rs"]
#[include = "Cargo.toml"]
struct RustRuntime;

/// Which targets this machine can execute, probed by the tool each scaffold
/// invokes. TypeScript is excluded: the browser half runs it locally.
pub fn available_targets() -> Vec<&'static str> {
    let mut targets = Vec::new();
    if probe("cargo") {
        targets.push("rust");
    }
    if probe("go") {
        targets.push("go");
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
    mocks: Mocks,
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

#[derive(serde::Serialize, Clone)]
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

fn execute_in(
    scratch: &std::path::Path,
    request: &RunRequest,
    kind: TargetKind,
) -> Result<Vec<Line>, String> {
    let source_path = scratch.join("playground.tono");
    std::fs::write(&source_path, &request.source).map_err(|e| e.to_string())?;
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
    let files = generate_target(&model, kind, &config, &casing_for(kind))?;

    let project = scratch.join(kind.dir());
    match kind {
        TargetKind::Rust => scaffold_rust(&files, &project, &request.snippet),
        TargetKind::Go => scaffold_go(&files, &project, &request.snippet),
        TargetKind::TypeScript => unreachable!("rejected above"),
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
        TargetKind::TypeScript => unreachable!(),
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

fn unpack<E: rust_embed::Embed>(root: &std::path::Path) -> std::io::Result<()> {
    for name in E::iter() {
        let file = E::get(&name).expect("embedded file iterates");
        let dest = root.join(name.as_ref());
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(dest, file.data)?;
    }
    Ok(())
}

/// A binary crate over the generated SDK: the generated `lib.rs` is kept (it
/// carries the module tree and re-exports), the snippet becomes `src/main.rs`,
/// and the embedded HTTP runtime is a path dependency. The first run downloads
/// the runtime's own dependencies from crates.io like any cargo project.
fn scaffold_rust(
    files: &[GeneratedFile],
    root: &std::path::Path,
    snippet: &str,
) -> std::io::Result<()> {
    let src = root.join("src");
    write_sources(files, TargetKind::Rust, &src)?;
    std::fs::write(src.join("main.rs"), snippet)?;
    unpack::<RustRuntime>(&root.join("_runtime/http-rust"))?;
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
         sdk-http-runtime-rs = { path = \"_runtime/http-rust\" }\n\
         [workspace]\n\
         exclude = [\"_runtime/http-rust\"]\n",
    )
}

/// A Go module over the generated packages: `main.go` is the snippet and the
/// embedded HTTP runtime satisfies the SDK's import through a local replace.
fn scaffold_go(
    files: &[GeneratedFile],
    root: &std::path::Path,
    snippet: &str,
) -> std::io::Result<()> {
    write_sources(files, TargetKind::Go, root)?;
    std::fs::write(root.join("main.go"), snippet)?;
    unpack::<GoRuntime>(&root.join("_runtime/http-go"))?;
    std::fs::write(
        root.join("go.mod"),
        format!(
            "module {GO_SCAFFOLD_MODULE}\n\ngo 1.21\n\n\
             require github.com/tono-lang/tono/runtimes/http-go v0.0.0\n\n\
             replace github.com/tono-lang/tono/runtimes/http-go => ./_runtime/http-go\n"
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
                    let hit = routes.get(&format!("{method} {path}")).cloned();
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

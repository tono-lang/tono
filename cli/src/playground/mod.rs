//! `tono playground`: the web playground served locally. The browser half is
//! the same app the hosted page ships (it compiles and previews on its own,
//! via the embedded jsoo/wasm compiler); this server adds what a browser
//! cannot do: execute code against the generated SDK with the machine's real
//! toolchains, behind `/api/run`. The trust model matches `tono preview`:
//! the user's own toolchains, no sandbox.

mod assets;
mod run;

use std::sync::Arc;

const USAGE: &str = "usage: tono playground [--port <n>] [--ui-dir <path>] [--no-open]";

struct Options {
    port: u16,
    ui_dir: Option<std::path::PathBuf>,
    open: bool,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut options = Options {
        port: 7690,
        ui_dir: None,
        open: true,
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--port" => {
                let value = it.next().ok_or(USAGE)?;
                options.port = value.parse().map_err(|_| format!("bad port: {value}"))?;
            }
            "--ui-dir" => {
                options.ui_dir = Some(std::path::PathBuf::from(it.next().ok_or(USAGE)?));
            }
            "--no-open" => options.open = false,
            _ => return Err(USAGE.to_string()),
        }
    }
    Ok(options)
}

pub fn run(args: &[String]) -> Result<(), String> {
    let options = parse(args)?;
    let assets = Arc::new(assets::Assets::new(options.ui_dir.clone())?);
    let addr = format!("127.0.0.1:{}", options.port);
    let server = tiny_http::Server::http(&addr).map_err(|e| format!("cannot bind {addr}: {e}"))?;
    let url = format!("http://{addr}/");
    println!("tono playground serving on {url}");
    if options.open {
        open_browser(&url);
    }
    for request in server.incoming_requests() {
        let assets = Arc::clone(&assets);
        // One thread per request: assets are instant, and a run occupies its
        // thread for as long as the target toolchain takes, without blocking
        // the UI's other calls.
        std::thread::spawn(move || handle(request, &assets));
    }
    Ok(())
}

fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let launcher = "open";
    #[cfg(not(target_os = "macos"))]
    let launcher = "xdg-open";
    let _ = std::process::Command::new(launcher)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

fn handle(mut request: tiny_http::Request, assets: &assets::Assets) {
    let method = request.method().clone();
    let path = request.url().split('?').next().unwrap_or("/").to_string();
    let response = match (method, path.as_str()) {
        (tiny_http::Method::Get, "/api/capabilities") => capabilities_response(),
        (tiny_http::Method::Post, "/api/run") => {
            let mut body = String::new();
            match request.as_reader().read_to_string(&mut body) {
                Ok(_) => run::handle(&body),
                Err(e) => json_error(400, &format!("unreadable body: {e}")),
            }
        }
        (tiny_http::Method::Get, _) => assets.serve(&path),
        _ => json_error(405, "method not allowed"),
    };
    let _ = request.respond(response);
}

pub(crate) type Response = tiny_http::Response<std::io::Cursor<Vec<u8>>>;

pub(crate) fn json_response(status: u16, body: serde_json::Value) -> Response {
    let bytes = body.to_string().into_bytes();
    tiny_http::Response::from_data(bytes)
        .with_status_code(status)
        .with_header(
            tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                .expect("static header"),
        )
}

pub(crate) fn json_error(status: u16, message: &str) -> Response {
    json_response(status, serde_json::json!({ "error": message }))
}

/// What this machine can do beyond the browser: which targets `/api/run` can
/// execute, detected by probing the toolchains the run scaffolds invoke.
/// TypeScript is absent on purpose: the browser runs it locally either way.
fn capabilities_response() -> Response {
    json_response(
        200,
        serde_json::json!({
            "version": tono_backend::version(),
            "runTargets": run::available_targets(),
        }),
    )
}

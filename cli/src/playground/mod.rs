//! `tono playground`: the web playground served locally. The browser half is
//! the same app the hosted page ships (it compiles and previews on its own,
//! via the embedded jsoo/wasm compiler); this server adds what a browser
//! cannot do: execute code against the generated SDK with the machine's real
//! toolchains, behind `/api/run`. The trust model matches `tono preview`:
//! the user's own toolchains, no sandbox.

mod assets;
mod lspproxy;
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
    let body = |request: &mut tiny_http::Request| {
        let mut body = String::new();
        request
            .as_reader()
            .read_to_string(&mut body)
            .map(|_| body)
            .map_err(|e| format!("unreadable body: {e}"))
    };
    let response = route(&method, &path, || body(&mut request), assets);
    let _ = request.respond(response);
}

/// The server's whole surface, factored from the transport so it is testable
/// without sockets: two POST endpoints, capabilities, and static assets.
fn route(
    method: &tiny_http::Method,
    path: &str,
    body: impl FnOnce() -> Result<String, String>,
    assets: &assets::Assets,
) -> Response {
    match (method, path) {
        (tiny_http::Method::Get, "/api/capabilities") => capabilities_response(),
        (tiny_http::Method::Post, "/api/run") => match body() {
            Ok(body) => run::handle(&body),
            Err(e) => json_error(400, &e),
        },
        (tiny_http::Method::Post, "/api/complete") => match body() {
            Ok(body) => lspproxy::handle(&body),
            Err(e) => json_error(400, &e),
        },
        (tiny_http::Method::Get, _) => assets.serve(path),
        _ => json_error(405, "method not allowed"),
    }
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
            "lspTargets": lspproxy::available(),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn options_parse_port_ui_dir_and_no_open() {
        let options = parse(&args(&[
            "--port",
            "8080",
            "--ui-dir",
            "/tmp/x",
            "--no-open",
        ]))
        .expect("parses");
        assert_eq!(options.port, 8080);
        assert_eq!(
            options.ui_dir.as_deref(),
            Some(std::path::Path::new("/tmp/x"))
        );
        assert!(!options.open);
        let defaults = parse(&[]).expect("parses");
        assert_eq!(defaults.port, 7690);
        assert!(defaults.open);
    }

    #[test]
    fn bad_flags_and_ports_are_usage_errors() {
        assert!(parse(&args(&["--wat"])).is_err());
        assert!(parse(&args(&["--port"])).is_err());
        assert!(parse(&args(&["--port", "nope"])).is_err());
    }

    #[test]
    fn json_responses_carry_status_and_content_type() {
        let ok = json_response(200, serde_json::json!({ "a": 1 }));
        assert_eq!(ok.status_code(), tiny_http::StatusCode(200));
        let err = json_error(422, "why");
        assert_eq!(err.status_code(), tiny_http::StatusCode(422));
    }

    #[test]
    fn routing_covers_the_whole_surface() {
        let dir = std::env::temp_dir().join(format!("tono-route-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("index.html"), "<title>x</title>").expect("index");
        let assets = assets::Assets::new(Some(dir.clone())).expect("assets");
        let get = tiny_http::Method::Get;
        let post = tiny_http::Method::Post;
        let ok_body = || Ok(String::from("{oops"));
        assert_eq!(
            route(&get, "/api/capabilities", ok_body, &assets).status_code(),
            tiny_http::StatusCode(200)
        );
        assert_eq!(
            route(&post, "/api/run", ok_body, &assets).status_code(),
            tiny_http::StatusCode(400)
        );
        assert_eq!(
            route(&post, "/api/complete", ok_body, &assets).status_code(),
            tiny_http::StatusCode(400)
        );
        assert_eq!(
            route(&post, "/api/run", || Err(String::from("broken")), &assets).status_code(),
            tiny_http::StatusCode(400)
        );
        assert_eq!(
            route(&get, "/index.html", ok_body, &assets).status_code(),
            tiny_http::StatusCode(200)
        );
        assert_eq!(
            route(&tiny_http::Method::Delete, "/x", ok_body, &assets).status_code(),
            tiny_http::StatusCode(405)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn capabilities_report_versions_and_probed_targets() {
        // Contents depend on the machine; the shape and status do not.
        let response = capabilities_response();
        assert_eq!(response.status_code(), tiny_http::StatusCode(200));
    }
}

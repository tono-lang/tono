//! The web app's static files: compiled into the binary under the `embed-ui`
//! feature (release builds), or served from a directory (`--ui-dir`, or the
//! repo's `playground/dist` during development) so plain cargo builds never
//! need node.

use super::Response;

#[cfg(feature = "embed-ui")]
#[derive(rust_embed::Embed)]
#[folder = "../playground/dist"]
struct EmbeddedUi;

pub struct Assets {
    dir: Option<std::path::PathBuf>,
}

impl Assets {
    pub fn new(dir: Option<std::path::PathBuf>) -> Result<Self, String> {
        let dir = match dir {
            Some(d) => Some(
                d.canonicalize()
                    .map_err(|e| format!("--ui-dir {}: {e}", d.display()))?,
            ),
            None => Self::default_dir(),
        };
        if dir.is_none() && !cfg!(feature = "embed-ui") {
            return Err(
                "no UI to serve: build playground/dist (npm run build) or pass --ui-dir"
                    .to_string(),
            );
        }
        Ok(Assets { dir })
    }

    /// A dev checkout's `playground/dist`, discovered relative to the running
    /// binary (target/{debug,release}/tono), so `cargo run -- playground`
    /// works without flags. Absent outside a checkout.
    fn default_dir() -> Option<std::path::PathBuf> {
        let exe = std::env::current_exe().ok()?;
        let repo = exe.parent()?.parent()?.parent()?;
        let dist = repo.join("playground").join("dist");
        dist.is_dir().then(|| dist.canonicalize().ok())?
    }

    pub fn serve(&self, path: &str) -> Response {
        let rel = path.trim_start_matches('/');
        let rel = if rel.is_empty() { "index.html" } else { rel };
        if let Some(bytes) = self.load(rel) {
            return raw(200, content_type(rel), bytes);
        }
        // A hash-routed SPA: anything unknown falls back to the page itself.
        match self.load("index.html") {
            Some(bytes) => raw(200, "text/html; charset=utf-8", bytes),
            None => raw(404, "text/plain", b"not found".to_vec()),
        }
    }

    fn load(&self, rel: &str) -> Option<Vec<u8>> {
        #[cfg(feature = "embed-ui")]
        if let Some(file) = EmbeddedUi::get(rel) {
            return Some(file.data.into_owned());
        }
        let dir = self.dir.as_ref()?;
        let full = dir.join(rel);
        // The canonical path must stay under the UI root; a traversal attempt
        // (../) escapes it and is refused.
        let full = full.canonicalize().ok()?;
        if !full.starts_with(dir) {
            return None;
        }
        std::fs::read(full).ok()
    }
}

fn raw(status: u16, content_type: &str, bytes: Vec<u8>) -> Response {
    tiny_http::Response::from_data(bytes)
        .with_status_code(status)
        .with_header(
            tiny_http::Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes())
                .expect("static header"),
        )
        // A local dev tool rebuilt in place: the browser must revalidate, or a
        // stale bundle shadows every fix.
        .with_header(
            tiny_http::Header::from_bytes(&b"Cache-Control"[..], &b"no-cache"[..])
                .expect("static header"),
        )
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" | "cjs" => "text/javascript",
        "css" => "text/css",
        "wasm" => "application/wasm",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

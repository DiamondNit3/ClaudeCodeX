use anyhow::{bail, Result};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub struct PreviewServer {
    workspace: PathBuf,
    port: u16,
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl PreviewServer {
    pub fn start(workspace: PathBuf) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let running = Arc::new(AtomicBool::new(true));
        let thread_running = running.clone();
        let thread_workspace = workspace.clone();
        let handle = thread::spawn(move || {
            while thread_running.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = handle_connection(stream, &thread_workspace);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(25));
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            workspace,
            port,
            running,
            handle: Some(handle),
        })
    }

    pub fn url_for(&self, path: &str) -> Result<String> {
        let resolved = resolve_workspace_path(&self.workspace, path)?;
        if !resolved.is_file() {
            bail!("preview file {} does not exist", resolved.display());
        }
        Ok(format!(
            "http://127.0.0.1:{}/{}",
            self.port,
            path.trim_start_matches(['/', '\\']).replace('\\', "/")
        ))
    }
}

impl Drop for PreviewServer {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn handle_connection(mut stream: TcpStream, workspace: &Path) -> Result<()> {
    let mut buffer = [0u8; 2048];
    let bytes = stream.read(&mut buffer)?;
    let request = String::from_utf8_lossy(&buffer[..bytes]);
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");
    let path = path.trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    let resolved = resolve_workspace_path(workspace, path)?;

    if resolved.is_file() {
        let body = fs::read(&resolved)?;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\n\r\n",
            content_type(&resolved),
            body.len()
        );
        stream.write_all(response.as_bytes())?;
        stream.write_all(&body)?;
    } else {
        let body = b"Not found";
        stream.write_all(
            format!(
                "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )?;
        stream.write_all(body)?;
    }
    Ok(())
}

fn resolve_workspace_path(workspace: &Path, path: &str) -> Result<PathBuf> {
    let relative = path.trim_start_matches(['/', '\\']);
    if Path::new(relative).components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        bail!("preview path is outside workspace");
    }
    let resolved = workspace.join(relative);
    let workspace = workspace.canonicalize()?;
    let candidate = if resolved.exists() {
        resolved.canonicalize()?
    } else {
        resolved
    };
    if candidate.starts_with(&workspace) {
        Ok(candidate)
    } else {
        bail!("preview path is outside workspace")
    }
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|value| value.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

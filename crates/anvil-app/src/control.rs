//! The control socket (`docs/APP.md` §3): the running phone, reachable by
//! the same user on the same machine — a Unix socket in a directory only
//! they can open, or a named pipe that refuses remote clients. One JSON
//! [`Command`] per line in, one [`Response`] per line out; after a
//! `subscribe`, one [`Change`](crate::Change) per line until the client
//! goes.

use std::path::PathBuf;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

use crate::{Command, Phone, Response};

/// Where a phone's control socket is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// A Unix socket (Linux, macOS).
    Unix(PathBuf),
    /// A named pipe (`\\.\pipe\…`, Windows).
    Pipe(String),
}

impl Endpoint {
    /// This user's default: `$XDG_RUNTIME_DIR/anvil/control.sock` (else the
    /// cache directory) on Linux and macOS, `\\.\pipe\anvil-<user>` on
    /// Windows.
    pub fn default_for_user() -> Self {
        if cfg!(windows) {
            let user = std::env::var("USERNAME").unwrap_or_else(|_| "user".into());
            Endpoint::Pipe(format!(r"\\.\pipe\anvil-{user}"))
        } else {
            let base = dirs::runtime_dir()
                .or_else(dirs::cache_dir)
                .unwrap_or_else(std::env::temp_dir);
            Endpoint::Unix(base.join("anvil").join("control.sock"))
        }
    }

    /// An endpoint named on the command line: a path, or a pipe name.
    pub fn named(name: &str) -> Self {
        if name.starts_with(r"\\.\pipe\") {
            Endpoint::Pipe(name.to_string())
        } else {
            Endpoint::Unix(PathBuf::from(name))
        }
    }
}

impl std::fmt::Display for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Endpoint::Unix(p) => write!(f, "{}", p.display()),
            Endpoint::Pipe(n) => f.write_str(n),
        }
    }
}

/// What can go wrong talking to a phone.
#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    #[error("no phone is running at {0} (start one with `anvil-cli run`)")]
    NotRunning(Endpoint),
    #[error("a phone is already running at {0}")]
    AlreadyRunning(Endpoint),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("the phone answered something unreadable: {0}")]
    Protocol(String),
}

/// Serve `phone` at `endpoint` until the task is dropped.
pub async fn serve(phone: Phone, endpoint: Endpoint) -> Result<(), ControlError> {
    match &endpoint {
        Endpoint::Unix(path) => serve_unix(phone, path.clone(), endpoint.clone()).await,
        Endpoint::Pipe(name) => serve_pipe(phone, name.clone(), endpoint.clone()).await,
    }
}

#[cfg(unix)]
async fn serve_unix(phone: Phone, path: PathBuf, endpoint: Endpoint) -> Result<(), ControlError> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    if let Some(dir) = path.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    }
    if path.exists() {
        // A socket that answers is a phone already running; one that does
        // not is left from a phone that stopped without cleaning up.
        if tokio::net::UnixStream::connect(&path).await.is_ok() {
            return Err(ControlError::AlreadyRunning(endpoint));
        }
        std::fs::remove_file(&path)?;
    }
    let listener = tokio::net::UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    let _cleanup = RemoveOnDrop(path);
    loop {
        let (stream, _) = listener.accept().await?;
        tokio::spawn(connection(phone.clone(), stream));
    }
}

#[cfg(not(unix))]
async fn serve_unix(_: Phone, _: PathBuf, endpoint: Endpoint) -> Result<(), ControlError> {
    Err(ControlError::Protocol(format!(
        "{endpoint} is a Unix socket; this platform uses named pipes"
    )))
}

#[cfg(windows)]
async fn serve_pipe(phone: Phone, name: String, endpoint: Endpoint) -> Result<(), ControlError> {
    use tokio::net::windows::named_pipe::ServerOptions;
    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .reject_remote_clients(true)
        .create(&name)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                ControlError::AlreadyRunning(endpoint.clone())
            } else {
                e.into()
            }
        })?;
    loop {
        server.connect().await?;
        let connected = server;
        server = ServerOptions::new()
            .reject_remote_clients(true)
            .create(&name)?;
        tokio::spawn(connection(phone.clone(), connected));
    }
}

#[cfg(not(windows))]
async fn serve_pipe(_: Phone, _: String, endpoint: Endpoint) -> Result<(), ControlError> {
    Err(ControlError::Protocol(format!(
        "{endpoint} is a named pipe; this platform uses Unix sockets"
    )))
}

#[cfg(unix)]
struct RemoveOnDrop(PathBuf);

#[cfg(unix)]
impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// One client: commands answered until it goes; after `subscribe`, the
/// phone's changes.
async fn connection<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(phone: Phone, stream: S) {
    let (read, mut write) = tokio::io::split(stream);
    let mut lines = BufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Command>(&line) {
            Ok(Command::Subscribe) => {
                if send(&mut write, &Response::ok()).await.is_err() {
                    return;
                }
                let mut changes = phone.changes();
                loop {
                    match changes.recv().await {
                        Ok(change) => {
                            if send(&mut write, &change).await.is_err() {
                                return;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(_) => return,
                    }
                }
            }
            Ok(command) => match phone.execute(command).await {
                Ok(response) => response,
                Err(e) => Response::error(e.to_string()),
            },
            Err(e) => Response::error(format!("not a command: {e}")),
        };
        if send(&mut write, &response).await.is_err() {
            return;
        }
    }
}

async fn send<W: AsyncWrite + Unpin>(
    write: &mut W,
    value: &impl serde::Serialize,
) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(value).map_err(std::io::Error::other)?;
    line.push(b'\n');
    write.write_all(&line).await?;
    write.flush().await
}

/// Open the pipe, waiting while every instance is busy: the server makes
/// its next instance only after the last client connected, so a client
/// arriving in between is told "busy" (`ERROR_PIPE_BUSY`) and tries again.
#[cfg(windows)]
async fn open_pipe(
    name: &str,
) -> std::io::Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    const ERROR_PIPE_BUSY: i32 = 231;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match tokio::net::windows::named_pipe::ClientOptions::new().open(name) {
            Err(e)
                if e.raw_os_error() == Some(ERROR_PIPE_BUSY)
                    && tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            other => return other,
        }
    }
}

/// A connection to a running phone.
pub struct Client {
    lines: tokio::io::Lines<BufReader<Box<dyn AsyncRead + Unpin + Send>>>,
    write: Box<dyn AsyncWrite + Unpin + Send>,
}

impl Client {
    /// Connect to the phone at `endpoint`.
    pub async fn connect(endpoint: &Endpoint) -> Result<Self, ControlError> {
        let not_running = |e: std::io::Error| match e.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
                ControlError::NotRunning(endpoint.clone())
            }
            _ => e.into(),
        };
        let (read, write): (
            Box<dyn AsyncRead + Unpin + Send>,
            Box<dyn AsyncWrite + Unpin + Send>,
        ) = match endpoint {
            #[cfg(unix)]
            Endpoint::Unix(path) => {
                let stream = tokio::net::UnixStream::connect(path)
                    .await
                    .map_err(not_running)?;
                let (r, w) = stream.into_split();
                (Box::new(r), Box::new(w))
            }
            #[cfg(windows)]
            Endpoint::Pipe(name) => {
                let pipe = open_pipe(name).await.map_err(not_running)?;
                let (r, w) = tokio::io::split(pipe);
                (Box::new(r), Box::new(w))
            }
            #[allow(unreachable_patterns)]
            _ => return Err(ControlError::NotRunning(endpoint.clone())),
        };
        Ok(Self {
            lines: BufReader::new(read).lines(),
            write,
        })
    }

    /// Send a command and read its answer.
    pub async fn request(&mut self, command: &Command) -> Result<Response, ControlError> {
        send(&mut self.write, command).await?;
        let line = self
            .lines
            .next_line()
            .await?
            .ok_or_else(|| ControlError::Protocol("the phone closed the connection".into()))?;
        serde_json::from_str(&line).map_err(|e| ControlError::Protocol(format!("{e}: {line}")))
    }

    /// Subscribe; then [`next_change`](Self::next_change) reads each change.
    pub async fn subscribe(&mut self) -> Result<(), ControlError> {
        let answer = self.request(&Command::Subscribe).await?;
        if answer.ok {
            Ok(())
        } else {
            Err(ControlError::Protocol(answer.error.unwrap_or_default()))
        }
    }

    /// The next change after [`subscribe`](Self::subscribe); `None` once the
    /// phone has gone.
    pub async fn next_change(&mut self) -> Result<Option<crate::Change>, ControlError> {
        match self.lines.next_line().await? {
            Some(line) => serde_json::from_str(&line)
                .map(Some)
                .map_err(|e| ControlError::Protocol(format!("{e}: {line}"))),
            None => Ok(None),
        }
    }
}

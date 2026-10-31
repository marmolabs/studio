use crate::api::{Request, Response};
use crate::studio::Studio;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

/// Answers API requests on the studio's socket until the listener fails.
pub async fn serve(studio: Arc<Studio>) -> std::io::Result<()> {
    let path = studio.socket_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // A socket file left by a previous run that didn't shut down cleanly.
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    // Only the owner may talk to the studio, even when the socket sits in /tmp.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    loop {
        let (stream, _) = listener.accept().await?;
        tokio::spawn(connection(Arc::clone(&studio), stream));
    }
}

async fn connection(studio: Arc<Studio>, stream: UnixStream) {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => studio.handle(request).await,
            Err(err) => Response::err(format!("bad request: {err}")),
        };
        let mut out = serde_json::to_string(&response)
            .unwrap_or_else(|_| r#"{"ok":false,"error":"could not encode reply"}"#.to_string());
        out.push('\n');
        if write.write_all(out.as_bytes()).await.is_err() {
            return;
        }
    }
}

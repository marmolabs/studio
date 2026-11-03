//! The README's photos: a Chromium-family browser, driven over DevTools.
//!
//! The browser's own `--screenshot` mode is no use here: in an agent's
//! sandbox it never wrote its file, even for a page with nothing to wait for
//! and with a virtual time budget. Driven over DevTools a headless browser
//! needs only a port on localhost. It is started with
//! `--remote-debugging-port=0`, says in its profile folder which port it took,
//! and takes the picture when asked, at the size wanted and once the page has
//! settled (the pages keep a live stream open, so there is no moment when they
//! are quiet). DevTools speaks JSON over a WebSocket; the little of one that
//! this needs is below, so no crate is added.

use crate::Result;
use crate::render::{base64, unbase64};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::process::Child;

/// A photo is the page in a window of this many CSS pixels, drawn this many
/// pixels wide: all a README photo needs, as it shows 900 wide. The window is
/// the narrowest that keeps the shop's newest line whole and the shortest that
/// shows its whole rail. The scale follows (1.875), so nothing is resized
/// afterwards: a photo resized to fit is a bigger file than one drawn that size.
const VIEW: (u32, u32) = (1280, 1000);
const WIDTH: u32 = 2400;

/// The longest any one step may take.
const STEP: Duration = Duration::from_secs(30);

/// How long a page has to settle once it has loaded: the stream that keeps it
/// up to date opens and draws it once, and the fonts arrive.
const SETTLE: Duration = Duration::from_millis(2500);

/// One photo: a page of a studio, saved as `file`.
pub struct Photo<'a> {
    pub file: &'a str,
    pub url: String,
}

/// Takes each photo in a fresh profile at `profile` and saves it in
/// `out_dir`; returns the paths written.
pub async fn take(
    browser: &Path,
    profile: &Path,
    photos: &[Photo<'_>],
    out_dir: &Path,
) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(out_dir)?;
    let mut devtools = Devtools::launch(browser, profile).await?;
    let written = take_each(&mut devtools, photos, out_dir).await;
    devtools.quit().await;
    written
}

async fn take_each(
    devtools: &mut Devtools,
    photos: &[Photo<'_>],
    out_dir: &Path,
) -> Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    for photo in photos {
        let png = devtools
            .photograph(photo)
            .await
            .map_err(|e| format!("{}: {e}", photo.file))?;
        let path = out_dir.join(photo.file);
        std::fs::write(&path, png)?;
        written.push(path);
    }
    Ok(written)
}

/// A browser, and the DevTools connection to it.
struct Devtools {
    child: Child,
    socket: Socket,
    next: u64,
    /// What came while a reply was awaited: events, kept for `event`.
    events: VecDeque<Value>,
}

impl Devtools {
    async fn launch(browser: &Path, profile: &Path) -> Result<Devtools> {
        let mut child = tokio::process::Command::new(browser)
            .args([
                "--headless=new",
                "--hide-scrollbars",
                "--no-first-run",
                "--no-default-browser-check",
                "--remote-debugging-port=0",
            ])
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", browser.display()))?;
        // Once the browser listens, its profile folder has a file with the
        // port it took and the address of its endpoint, a line each.
        let file = profile.join("DevToolsActivePort");
        let started = Instant::now();
        let (port, path) = loop {
            let text = std::fs::read_to_string(&file).unwrap_or_default();
            let mut lines = text.lines();
            if let (Some(port), Some(path)) = (lines.next(), lines.next())
                && let Ok(port) = port.trim().parse::<u16>()
            {
                break (port, path.trim().to_string());
            }
            if let Some(status) = child.try_wait()? {
                return Err(format!("the browser stopped at once: {status}").into());
            }
            if started.elapsed() > STEP {
                return Err("the browser never said where it listens".into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        let socket = tokio::time::timeout(STEP, Socket::connect(port, &path))
            .await
            .map_err(|_| "the browser did not answer on its DevTools port")??;
        Ok(Devtools {
            child,
            socket,
            next: 0,
            events: VecDeque::new(),
        })
    }

    /// Asks `method` of the page of `session`, or of the browser when there
    /// is none, and returns the answer.
    async fn call(&mut self, session: Option<&str>, method: &str, params: Value) -> Result<Value> {
        self.next += 1;
        let id = self.next;
        let mut request = json!({"id": id, "method": method, "params": params});
        if let Some(session) = session {
            request["sessionId"] = json!(session);
        }
        self.socket.send(&request.to_string()).await?;
        let reply = tokio::time::timeout(STEP, self.reply(id))
            .await
            .map_err(|_| format!("{method} took longer than {}s", STEP.as_secs()))??;
        match reply.get("error") {
            Some(error) => Err(format!("{method}: {error}").into()),
            None => Ok(reply["result"].clone()),
        }
    }

    /// The reply to request `id`. What comes before it is kept.
    async fn reply(&mut self, id: u64) -> Result<Value> {
        loop {
            let message: Value = serde_json::from_str(&self.socket.receive().await?)?;
            if message["id"] == id {
                return Ok(message);
            }
            self.events.push_back(message);
        }
    }

    /// Waits for the event `method` of the page of `session`.
    async fn event(&mut self, session: &str, method: &str) -> Result<()> {
        tokio::time::timeout(STEP, async {
            loop {
                let seen = |m: &Value| m["method"] == method && m["sessionId"] == session;
                if let Some(at) = self.events.iter().position(seen) {
                    self.events.remove(at);
                    return Ok(());
                }
                let message = serde_json::from_str(&self.socket.receive().await?)?;
                self.events.push_back(message);
            }
        })
        .await
        .map_err(|_| format!("no {method} within {}s", STEP.as_secs()))?
    }

    /// The photo as PNG bytes: the page in a tab of its own, showing the dark
    /// theme and holding still.
    async fn photograph(&mut self, photo: &Photo<'_>) -> Result<Vec<u8>> {
        let target = self
            .call(None, "Target.createTarget", json!({"url": "about:blank"}))
            .await?;
        let target = target["targetId"].as_str().ok_or("no target")?.to_string();
        let attached = self
            .call(
                None,
                "Target.attachToTarget",
                json!({"targetId": target, "flatten": true}),
            )
            .await?;
        let session = attached["sessionId"].as_str().ok_or("no session")?;
        let tab = Some(session);
        self.call(tab, "Page.enable", json!({})).await?;
        let scale = f64::from(WIDTH) / f64::from(VIEW.0);
        let view = json!({
            "width": VIEW.0, "height": VIEW.1,
            "deviceScaleFactor": scale, "mobile": false,
        });
        self.call(tab, "Emulation.setDeviceMetricsOverride", view)
            .await?;
        let media = json!({"features": [
            {"name": "prefers-color-scheme", "value": "dark"},
            {"name": "prefers-reduced-motion", "value": "reduce"},
        ]});
        self.call(tab, "Emulation.setEmulatedMedia", media).await?;
        // The blank tab's own load is not the page's.
        self.events.clear();
        let loading = self
            .call(tab, "Page.navigate", json!({"url": photo.url}))
            .await?;
        if let Some(why) = loading.get("errorText") {
            return Err(format!("{} would not load: {why}", photo.url).into());
        }
        self.event(session, "Page.loadEventFired").await?;
        self.run(session, "document.fonts.ready.then(() => true)")
            .await?;
        tokio::time::sleep(SETTLE).await;
        let shot = self
            .call(tab, "Page.captureScreenshot", json!({"format": "png"}))
            .await?;
        self.call(None, "Target.closeTarget", json!({"targetId": target}))
            .await?;
        let png = unbase64(shot["data"].as_str().ok_or("no picture")?)?;
        let wanted = (WIDTH, (f64::from(VIEW.1) * scale).round() as u32);
        match png_size(&png) {
            Some(size) if size == wanted => Ok(png),
            size => Err(format!("the picture is {size:?}, not {wanted:?}").into()),
        }
    }

    /// Runs `script` in the page, and waits for it if it is a promise.
    async fn run(&mut self, session: &str, script: &str) -> Result<()> {
        let params = json!({"expression": script, "awaitPromise": true});
        let done = self.call(Some(session), "Runtime.evaluate", params).await?;
        match done.get("exceptionDetails") {
            Some(why) => Err(format!("the page's script failed: {why}").into()),
            None => Ok(()),
        }
    }

    /// Closes the browser, and stops it if it will not close.
    async fn quit(mut self) {
        let _ = self.call(None, "Browser.close", json!({})).await;
        if tokio::time::timeout(Duration::from_secs(5), self.child.wait())
            .await
            .is_err()
        {
            let _ = self.child.kill().await;
        }
    }
}

/// The width and height a PNG says it has.
fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    let header = png.get(..24)?;
    if !header.starts_with(b"\x89PNG\r\n\x1a\n") {
        return None;
    }
    let side = |at: usize| {
        u32::from_be_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]])
    };
    Some((side(16), side(20)))
}

/// The key every handshake sends; a server only echoes it back, and nothing
/// here depends on it being random.
const KEY: &[u8; 16] = b"studio gallery!!";

/// The mask the protocol wants over what a client sends.
const MASK: [u8; 4] = [0x37, 0xfa, 0x21, 0x3d];

/// A WebSocket to a browser on this machine: text messages in both ways.
struct Socket {
    stream: TcpStream,
}

impl Socket {
    async fn connect(port: u16, path: &str) -> Result<Socket> {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await?;
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\n\
             Connection: Upgrade\r\nSec-WebSocket-Key: {}\r\nSec-WebSocket-Version: 13\r\n\r\n",
            base64(KEY)
        );
        stream.write_all(request.as_bytes()).await?;
        // The reply's headers end at a blank line, and the server says
        // nothing more until it is spoken to.
        let mut reply = Vec::new();
        while !reply.ends_with(b"\r\n\r\n") {
            reply.push(stream.read_u8().await?);
        }
        let reply = String::from_utf8_lossy(&reply);
        let status = reply.lines().next().unwrap_or_default();
        if !status.contains(" 101 ") {
            return Err(format!("the browser would not talk: {status}").into());
        }
        Ok(Socket { stream })
    }

    async fn send(&mut self, text: &str) -> Result<()> {
        self.stream.write_all(&frame(text)).await?;
        Ok(())
    }

    async fn receive(&mut self) -> Result<String> {
        message(&mut self.stream).await
    }
}

/// `text` as one masked frame, which is how a client sends it.
fn frame(text: &str) -> Vec<u8> {
    let payload = text.as_bytes();
    let mut out = vec![0x81];
    match payload.len() {
        n if n < 126 => out.push(0x80 | n as u8),
        n if n <= usize::from(u16::MAX) => {
            out.push(0x80 | 126);
            out.extend((n as u16).to_be_bytes());
        }
        n => {
            out.push(0x80 | 127);
            out.extend((n as u64).to_be_bytes());
        }
    }
    out.extend(MASK);
    out.extend(payload.iter().zip(MASK.iter().cycle()).map(|(b, m)| b ^ m));
    out
}

/// The next text message from a server, put together from its frames.
/// Anything else, such as a ping, is let go by.
async fn message<R: AsyncRead + Unpin>(from: &mut R) -> Result<String> {
    let mut text = Vec::new();
    loop {
        let first = from.read_u8().await?;
        let second = from.read_u8().await?;
        let len = match second & 0x7f {
            126 => u64::from(from.read_u16().await?),
            127 => from.read_u64().await?,
            n => u64::from(n),
        };
        // A server does not mask, but a frame that is masked is still read.
        let mask = if second & 0x80 == 0 {
            None
        } else {
            Some(from.read_u32().await?.to_be_bytes())
        };
        let mut payload = vec![0; usize::try_from(len)?];
        from.read_exact(&mut payload).await?;
        if let Some(mask) = mask {
            for (byte, mask) in payload.iter_mut().zip(mask.iter().cycle()) {
                *byte ^= mask;
            }
        }
        match first & 0x0f {
            // Text, or what goes on with it.
            0x0 | 0x1 => text.extend(payload),
            0x8 => return Err("the browser closed the connection".into()),
            _ => continue,
        }
        if first & 0x80 != 0 {
            return Ok(String::from_utf8(text)?);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[tokio::test]
    async fn a_masked_frame_reads_back_as_the_text_it_was_made_from() {
        // A short message and one long enough for the 64-bit length.
        for text in ["{\"id\":1}".to_string(), "x".repeat(70_000)] {
            let sent = frame(&text);
            assert_eq!(sent[0], 0x81);
            assert_ne!(sent[1] & 0x80, 0, "a client's frames are masked");
            assert_eq!(message(&mut &sent[..]).await.unwrap(), text);
        }
    }

    #[tokio::test]
    async fn a_message_comes_together_from_its_frames_and_a_ping_is_let_go() {
        let bytes = [
            &[0x89, 0x00][..],               // a ping
            &[0x01, 0x03, b'a', b'b', b'c'], // text, with more to come
            &[0x80, 0x02, b'd', b'e'],       // and the rest of it
        ]
        .concat();
        assert_eq!(message(&mut &bytes[..]).await.unwrap(), "abcde");
    }

    #[test]
    fn reads_a_png_size_and_base64_both_ways() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend(2880u32.to_be_bytes());
        png.extend(1800u32.to_be_bytes());
        assert_eq!(png_size(&png), Some((2880, 1800)));
        assert_eq!(png_size(b"GIF89a"), None);
        for bytes in [&b""[..], b"a", b"ab", b"abc", b"\0\xff\x10 hello"] {
            assert_eq!(unbase64(&base64(bytes)).unwrap(), bytes);
        }
    }
}

//! External clipboard read/write, selected by session type.
//!
//! GNOME's Mutter does not implement the wlroots data-control protocol, so
//! `wl-paste --watch` and GDK background monitoring are unreliable for an
//! unfocused daemon. Shelling out to `wl-paste`/`xclip` works regardless of
//! focus, so Cliccy reads (driven by XFIXES events) and writes through them.
//!
//! Supports plain text and PNG images. Images are preferred when both are
//! offered (e.g. a screenshot also exposing a file path as text).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::process::{Command, Stdio};

#[derive(Clone, Copy)]
pub enum Backend {
    Wayland,
    X11,
}

/// A clipboard payload Cliccy understands.
pub enum ClipContent {
    Text(String),
    /// PNG-encoded image bytes.
    Image(Vec<u8>),
}

const PNG_MAGIC: [u8; 8] = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

/// Pick a clipboard backend from the environment.
///
/// Prefer the X backend (`xclip`) whenever `DISPLAY` is available — including
/// under XWayland on GNOME — because it reads via Mutter's clipboard bridge
/// without grabbing Wayland focus. Native `wl-paste` reads create a
/// focus-grabbing surface on GNOME, jittering the active window. Pure-Wayland
/// (no `DISPLAY`) falls back to `wl-clipboard`.
pub fn detect() -> Backend {
    if std::env::var_os("DISPLAY").is_some() {
        Backend::X11
    } else if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        Backend::Wayland
    } else {
        Backend::X11
    }
}

/// Stable key for de-duplicating clipboard entries (text by value, image by hash).
pub fn dedup_key(content: &ClipContent) -> String {
    match content {
        ClipContent::Text(t) => t.clone(),
        ClipContent::Image(bytes) => image_key(bytes),
    }
}

/// Hash-based identity for an image, used both for change detection and storage
/// de-duplication so the same image isn't recorded twice.
pub fn image_key(bytes: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("img:{:016x}", hasher.finish())
}

impl Backend {
    /// Read the clipboard, preferring a PNG image over text.
    pub fn read(self) -> Option<ClipContent> {
        if let Some(bytes) = self.read_bytes("image/png") {
            if bytes.len() >= PNG_MAGIC.len() && bytes[..PNG_MAGIC.len()] == PNG_MAGIC {
                return Some(ClipContent::Image(bytes));
            }
        }
        let text = self.read_text()?;
        if text.is_empty() {
            None
        } else {
            Some(ClipContent::Text(text))
        }
    }

    /// Place a payload on the clipboard.
    pub fn write(self, content: &ClipContent) {
        match content {
            ClipContent::Text(t) => self.write_bytes(None, t.as_bytes()),
            ClipContent::Image(b) => self.write_bytes(Some("image/png"), b),
        }
    }

    fn read_text(self) -> Option<String> {
        let (cmd, args): (&str, &[&str]) = match self {
            // `-n` strips the trailing newline; `-t text` matches any text/* type.
            Backend::Wayland => ("wl-paste", &["-n", "-t", "text"]),
            Backend::X11 => ("xclip", &["-selection", "clipboard", "-o"]),
        };
        let output = Command::new(cmd).args(args).output().ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    /// Read the clipboard as raw bytes of a specific MIME type, or `None` if that
    /// type isn't offered. No `-n` here — newline stripping would corrupt binary.
    fn read_bytes(self, mime: &str) -> Option<Vec<u8>> {
        let output = match self {
            Backend::Wayland => Command::new("wl-paste").args(["-t", mime]).output().ok()?,
            Backend::X11 => Command::new("xclip")
                .args(["-selection", "clipboard", "-t", mime, "-o"])
                .output()
                .ok()?,
        };
        if !output.status.success() || output.stdout.is_empty() {
            return None;
        }
        Some(output.stdout)
    }

    fn write_bytes(self, mime: Option<&str>, data: &[u8]) {
        let mut command = match self {
            Backend::Wayland => Command::new("wl-copy"),
            Backend::X11 => {
                let mut c = Command::new("xclip");
                c.args(["-selection", "clipboard"]);
                c
            }
        };
        if let Some(m) = mime {
            command.args(["-t", m]);
        }
        let Ok(mut child) = command.stdin(Stdio::piped()).spawn() else {
            return;
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(data);
        }
        let _ = child.wait();
    }
}

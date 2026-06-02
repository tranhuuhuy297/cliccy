//! External clipboard read/write, selected by session type.
//!
//! GNOME's Mutter does not implement the wlroots data-control protocol, so
//! `wl-paste --watch` and GDK background monitoring are unreliable for an
//! unfocused daemon. Shelling out to `wl-paste`/`xclip` works regardless of
//! focus, so Cliccy polls for reads and uses `wl-copy`/`xclip` for writes.

use std::io::Write;
use std::process::{Command, Stdio};

#[derive(Clone, Copy)]
pub enum Backend {
    Wayland,
    X11,
}

/// Pick a clipboard backend from the environment.
///
/// We prefer the X backend (`xclip`) whenever `DISPLAY` is available — including
/// under XWayland on GNOME — because reading the X selection goes through
/// Mutter's clipboard bridge without grabbing Wayland focus. Native `wl-paste`
/// reads, by contrast, create a focus-grabbing surface on GNOME, which makes the
/// active window jitter on every poll. Pure-Wayland (no `DISPLAY`) falls back to
/// `wl-clipboard`.
pub fn detect() -> Backend {
    if std::env::var_os("DISPLAY").is_some() {
        Backend::X11
    } else if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        Backend::Wayland
    } else {
        Backend::X11
    }
}

impl Backend {
    /// Read the current clipboard text, or `None` if empty / non-text / failed.
    pub fn read(self) -> Option<String> {
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

    /// Place `text` on the clipboard.
    pub fn write(self, text: &str) {
        let (cmd, args): (&str, &[&str]) = match self {
            Backend::Wayland => ("wl-copy", &[]),
            Backend::X11 => ("xclip", &["-selection", "clipboard"]),
        };
        let Ok(mut child) = Command::new(cmd).args(args).stdin(Stdio::piped()).spawn() else {
            return;
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = child.wait();
    }
}

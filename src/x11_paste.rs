//! Paste a picked entry at the cursor by synthesising a paste keystroke after
//! the popup closes.
//!
//! Why `ydotool`: cliccy renders under XWayland, but the apps a user pastes
//! *into* are often native Wayland (browsers, GNOME apps). X11 input injection
//! (XTEST, `xdotool`) is invisible to those — it only reaches XWayland clients —
//! so an X11-only paste works in a terminal but silently fails in a browser.
//! `ydotool` injects through the kernel `uinput` device, *below* the display
//! server, so the keystroke reaches whatever has focus regardless of X11 vs
//! Wayland.
//!
//! Why two chords: GUI apps (browsers, editors) paste with `Ctrl+V`, but
//! terminals treat `Ctrl+V` as a literal control char and paste with
//! `Ctrl+Shift+V` instead — and `Ctrl+Shift+V` does *not* paste in a GUI field,
//! so no single chord serves both. We therefore detect whether the window that
//! was focused before the popup opened is a terminal (by its X11 `WM_CLASS`) and
//! pick the matching chord. Terminals are XWayland/X11 clients so their class is
//! readable; anything unreadable (a native-Wayland browser) falls through to the
//! `Ctrl+V` default, which is correct for it.
//!
//! Setup `ydotool` needs: read/write on `/dev/uinput`, granted by a udev rule
//! that puts the device in the `input` group plus the user being in `input`.
//! When `ydotool` is missing or can't open the device it simply no-ops, leaving
//! the entry on the clipboard for a manual paste (the prior behaviour).

use std::process::Command;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};

/// `WM_CLASS` substrings (lower-cased) that mark the focused window as a
/// terminal, which pastes with `Ctrl+Shift+V` rather than `Ctrl+V`.
const TERMINAL_CLASSES: &[&str] = &[
    "termius", "gnome-terminal", "konsole", "xterm", "alacritty", "kitty",
    "terminator", "tilix", "urxvt", "rxvt", "wezterm", "foot", "hyper",
    "guake", "yakuake", "ptyxis", "blackbox", "contour", "rio", "terminal",
];

/// Whether the currently active (focused) window looks like a terminal, read
/// from its X11 `WM_CLASS`. `false` when there's no X connection, no active
/// window, or the class can't be read — so a native-Wayland app (browser) takes
/// the `Ctrl+V` path. Call this *before* the popup maps, while the user's target
/// window still holds focus.
pub fn active_is_terminal() -> bool {
    let Some(class) = active_window_class() else {
        return false;
    };
    let class = class.to_lowercase();
    TERMINAL_CLASSES.iter().any(|t| class.contains(t))
}

/// The `WM_CLASS` of the EWMH active window (its instance+class strings joined),
/// or `None` under pure Wayland / when it can't be resolved.
fn active_window_class() -> Option<String> {
    let (conn, screen_num) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots[screen_num].root;
    let active = atom(&conn, b"_NET_ACTIVE_WINDOW")?;
    let reply = conn
        .get_property(false, root, active, AtomEnum::WINDOW, 0, 1)
        .ok()?
        .reply()
        .ok()?;
    let win = reply.value32()?.next()?;
    if win == 0 {
        return None;
    }
    let class = conn
        .get_property(false, win, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 256)
        .ok()?
        .reply()
        .ok()?;
    if class.value.is_empty() {
        return None;
    }
    // WM_CLASS is two NUL-separated strings (instance, class); keep both.
    Some(String::from_utf8_lossy(&class.value).replace('\0', " "))
}

/// Send the appropriate paste chord to the focused window via `ydotool`:
/// `Ctrl+Shift+V` for terminals, `Ctrl+V` for everything else. The app then
/// pastes from the clipboard cliccy just set (so capitals, unicode and images
/// all work). Spawned detached so it never blocks the GTK main thread. The
/// named-key form is what this ydotool understands.
pub fn send_paste(is_terminal: bool) {
    let chord = if is_terminal { "ctrl+shift+v" } else { "ctrl+v" };
    let _ = Command::new("ydotool").args(["key", chord]).spawn();
}

fn atom<C: Connection>(conn: &C, name: &[u8]) -> Option<u32> {
    conn.intern_atom(false, name).ok()?.reply().ok().map(|r| r.atom)
}

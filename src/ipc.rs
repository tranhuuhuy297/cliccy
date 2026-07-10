//! Unix-socket control channel between a `cliccy toggle` invocation and the
//! resident daemon.
//!
//! Why this exists: on Wayland, apps cannot grab a global hotkey, so the hotkey
//! is a GNOME custom keybinding that runs `cliccy toggle`. GNOME launches that
//! command through a systemd *transient scope*, and scope creation can stall for
//! seconds when the user systemd manager is cold — the "slow sometimes" the
//! popup exhibits. The GlobalShortcuts portal that would let the daemon own the
//! key directly is absent before GNOME 48, so we can't avoid the spawn; we make
//! the spawned process trivial instead.
//!
//! The previous forward path made `cliccy toggle` a *second GApplication
//! instance*: it initialised GTK, opened the D-Bus session bus, registered under
//! the app id, forwarded the verb to the primary instance, then exited. This
//! replaces that with a one-line socket write — no GTK, no bus registration — so
//! the process GNOME spawns does almost nothing and exits at once. The daemon
//! listens on a background thread and forwards each verb to the glib main loop
//! over the same `async_channel` the tray uses (see `tray.rs`).

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::time::Duration;

use gtk::glib;

use crate::app::Shared;
use crate::{config, ui};

/// Verbs the client may send. One short line per connection.
const TOGGLE: &str = "toggle";
const SHOW: &str = "show";
const HIDE: &str = "hide";

/// Client side: try to hand `verb` to a running daemon over the control socket.
///
/// Returns `true` when a daemon accepted the verb (the normal case — the popup
/// is being driven), `false` when no daemon is listening so the caller should
/// fall back to becoming/forwarding via the GApplication path. Kept fully
/// synchronous and GTK-free: this runs in the throwaway process GNOME spawns for
/// the hotkey, and its whole job is to be cheap.
pub fn try_forward(verb: &str) -> bool {
    let path = config::socket_path();
    // A short connect timeout isn't available on `UnixStream::connect`; connect
    // is effectively instant for a local socket that exists, and returns an error
    // immediately when it doesn't — which is exactly the "no daemon" signal.
    let Ok(mut stream) = UnixStream::connect(&path) else {
        return false;
    };
    // Bound the write/read so a wedged daemon can't hang the hotkey process.
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    if stream.write_all(verb.as_bytes()).is_err() || stream.write_all(b"\n").is_err() {
        return false;
    }
    let _ = stream.flush();
    // Read a one-byte ack so we only report success once the daemon has actually
    // received the verb (not merely buffered it). A missing ack still means the
    // daemon has our bytes; treat any successful connect+write as delivered.
    let mut ack = [0u8; 1];
    let _ = stream.read(&mut ack);
    true
}

/// Daemon side: bind the control socket and forward incoming verbs to the UI.
///
/// Runs the blocking `accept` loop on a dedicated thread (like the tray's D-Bus
/// service) and marshals each verb onto the glib main thread via `async_channel`,
/// because the UI and store are single-threaded. Called once from daemon
/// startup. A bind failure (e.g. the runtime dir is read-only) is logged and
/// otherwise ignored — the tray and any future GApplication forward still work.
pub fn install(shared: &Shared) {
    let path = config::socket_path();
    // Clear a socket left by a daemon that didn't shut down cleanly; without this
    // `bind` fails with EADDRINUSE on the stale file. Safe: if a live daemon
    // still holds it, our own `install` wouldn't be running (single instance).
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[cliccy] control socket unavailable ({e}); hotkey falls back to slow path");
            return;
        }
    };

    let (tx, rx) = async_channel::bounded::<String>(8);

    std::thread::Builder::new()
        .name("cliccy-ipc".into())
        .spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut stream) = conn else { continue };
                let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
                let mut buf = Vec::with_capacity(16);
                let mut byte = [0u8; 1];
                // Read one line (verbs are short); stop at newline or EOF.
                while let Ok(1) = stream.read(&mut byte) {
                    if byte[0] == b'\n' || buf.len() >= 32 {
                        break;
                    }
                    buf.push(byte[0]);
                }
                let verb = String::from_utf8_lossy(&buf).trim().to_string();
                if verb.is_empty() {
                    continue;
                }
                // Hand off before acking so the client's success reflects that the
                // main loop will act on it. `send_blocking` only parks if the main
                // loop is wedged, which would make any path slow anyway.
                if tx.send_blocking(verb).is_ok() {
                    let _ = stream.write_all(b"1");
                }
            }
        })
        .ok();

    // Receiver on the glib main thread: drive the UI exactly as the tray does.
    let shared = shared.clone();
    glib::spawn_future_local(async move {
        while let Ok(verb) = rx.recv().await {
            match verb.as_str() {
                TOGGLE => ui::toggle(&shared),
                SHOW => ui::show(&shared),
                HIDE => ui::hide(&shared),
                _ => {}
            }
        }
    });
}

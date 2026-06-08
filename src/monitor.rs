//! Clipboard monitoring.
//!
//! Preferred path is event-driven via X11 XFIXES (see `x11_clipboard_watch`),
//! which fires only on real clipboard changes — no idle CPU, no focus jitter.
//! If X is unavailable (pure-Wayland with no XWayland), we fall back to timer
//! polling, which still works but spawns a reader each tick.

use std::time::Duration;

use gtk::prelude::WidgetExt;
use gtk::{gio, glib};

use crate::app::Shared;
use crate::clipboard_backend::{self, ClipContent};
use crate::{ui, x11_clipboard_watch};

/// Fallback poll interval, used only when XFIXES watching is unavailable.
const POLL_INTERVAL: Duration = Duration::from_millis(600);

pub fn install(state: &Shared) {
    if x11_clipboard_watch::try_install(state) {
        return;
    }
    eprintln!("[cliccy] XFIXES unavailable; falling back to clipboard polling");
    install_polling(state);
}

/// Read the current clipboard and store it if it's new. Shared by the XFIXES
/// watcher and the polling fallback.
///
/// The `xclip` / `wl-paste` read blocks on a subprocess. Running it inline on the
/// GTK main thread stalls everything else that thread serves — including the
/// `cliccy toggle` forward that shows the popup — so a copy immediately followed
/// by the hotkey (a common flow) could delay the popup until the read finished.
/// Offload the read to a worker thread and resume on the main thread to touch the
/// single-threaded store and UI, keeping the popup responsive during a capture.
pub fn capture(state: &Shared) {
    let backend = state.backend;
    let state = state.clone();
    glib::spawn_future_local(async move {
        let Ok(Some(content)) = gio::spawn_blocking(move || backend.read()).await else {
            return;
        };
        let key = clipboard_backend::dedup_key(&content);
        if state.last_seen.borrow().as_deref() == Some(key.as_str()) {
            return;
        }
        *state.last_seen.borrow_mut() = Some(key);

        match content {
            ClipContent::Text(t) => {
                let _ = state.store.record_text(&t);
            }
            ClipContent::Image(bytes) => {
                let _ = state.store.record_image(&bytes);
            }
        }
        if state.window.is_visible() {
            ui::refresh(&state);
        }
    });
}

fn install_polling(state: &Shared) {
    let state = state.clone();
    glib::timeout_add_local(POLL_INTERVAL, move || {
        capture(&state);
        glib::ControlFlow::Continue
    });
}

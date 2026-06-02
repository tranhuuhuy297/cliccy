//! Clipboard monitoring.
//!
//! Preferred path is event-driven via X11 XFIXES (see `x11_clipboard_watch`),
//! which fires only on real clipboard changes — no idle CPU, no focus jitter.
//! If X is unavailable (pure-Wayland with no XWayland), we fall back to timer
//! polling, which still works but spawns a reader each tick.

use std::time::Duration;

use gtk::glib;
use gtk::prelude::WidgetExt;

use crate::app::Shared;
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

fn install_polling(state: &Shared) {
    let state = state.clone();
    glib::timeout_add_local(POLL_INTERVAL, move || {
        if let Some(text) = state.backend.read() {
            let is_new = state.last_seen.borrow().as_deref() != Some(text.as_str());
            if is_new {
                *state.last_seen.borrow_mut() = Some(text.clone());
                if !text.trim().is_empty() {
                    let _ = state.store.record(&text);
                    if state.window.is_visible() {
                        ui::refresh(&state);
                    }
                }
            }
        }
        glib::ControlFlow::Continue
    });
}

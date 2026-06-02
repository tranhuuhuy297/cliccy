//! Event-driven clipboard-change detection via X11 XFIXES.
//!
//! Under XWayland on GNOME, Mutter mirrors the Wayland selection onto the X
//! CLIPBOARD and takes ownership when it changes, which raises an XFIXES
//! "selection owner changed" event. Listening for that lets Cliccy read the
//! clipboard only when it actually changes — no timer, no per-tick process
//! spawning, and therefore none of the focus jitter polling caused.
//!
//! The X connection's socket is registered with the GLib main loop, so events
//! are handled inline on the UI thread without a background thread.

use std::os::fd::AsRawFd;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::WidgetExt;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xfixes::{self, SelectionEventMask};
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::protocol::Event;

use crate::app::Shared;
use crate::ui;

/// Attempt to start XFIXES watching. Returns `false` if X is unreachable or the
/// extension is missing, so the caller can fall back to polling.
pub fn try_install(state: &Shared) -> bool {
    let Ok((conn, screen_num)) = x11rb::connect(None) else {
        return false;
    };
    let conn = Rc::new(conn);

    if conn
        .extension_information(xfixes::X11_EXTENSION_NAME)
        .ok()
        .flatten()
        .is_none()
    {
        return false;
    }
    let version_ok = match xfixes::query_version(conn.as_ref(), 5, 0) {
        Ok(cookie) => cookie.reply().is_ok(),
        Err(_) => false,
    };
    if !version_ok {
        return false;
    }

    let Some(clipboard) = intern_atom(conn.as_ref(), b"CLIPBOARD") else {
        return false;
    };
    let root = conn.setup().roots[screen_num].root;
    let mask = SelectionEventMask::SET_SELECTION_OWNER
        | SelectionEventMask::SELECTION_WINDOW_DESTROY
        | SelectionEventMask::SELECTION_CLIENT_CLOSE;
    if xfixes::select_selection_input(conn.as_ref(), root, clipboard, mask).is_err() {
        return false;
    }
    if conn.flush().is_err() {
        return false;
    }

    let fd = conn.stream().as_raw_fd();
    let conn_cb = conn.clone();
    let state_cb = state.clone();
    glib::source::unix_fd_add_local(fd, glib::IOCondition::IN, move |_, _| {
        // Drain every buffered event; a single read covers any burst of changes.
        let mut changed = false;
        while let Ok(Some(event)) = conn_cb.poll_for_event() {
            if matches!(event, Event::XfixesSelectionNotify(_)) {
                changed = true;
            }
        }
        if changed {
            capture(&state_cb);
        }
        glib::ControlFlow::Continue
    });

    // Record whatever is already on the clipboard at startup.
    capture(state);
    true
}

/// Read the current clipboard and store it if it is new, non-empty text.
fn capture(state: &Shared) {
    let Some(text) = state.backend.read() else {
        return;
    };
    let is_new = state.last_seen.borrow().as_deref() != Some(text.as_str());
    if !is_new || text.trim().is_empty() {
        return;
    }
    *state.last_seen.borrow_mut() = Some(text.clone());
    let _ = state.store.record(&text);
    if state.window.is_visible() {
        ui::refresh(state);
    }
}

fn intern_atom(conn: &impl Connection, name: &[u8]) -> Option<u32> {
    conn.intern_atom(false, name)
        .ok()?
        .reply()
        .ok()
        .map(|r| r.atom)
}

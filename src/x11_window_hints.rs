//! XWayland window hints that make the popup behave like a global-hotkey
//! launcher on GNOME.
//!
//! Native Wayland windows can't opt out of the taskbar or force themselves to
//! the front, so Cliccy renders under XWayland (`GDK_BACKEND=x11`) and sets the
//! EWMH properties Mutter honours:
//!
//! - `_NET_WM_WINDOW_TYPE_UTILITY` + `SKIP_TASKBAR`/`SKIP_PAGER` keep it out of
//!   the dock (no dock icon, no reflow "jerk").
//! - `_NET_WM_STATE_ABOVE` keeps it on top so a hotkey-spawned popup is visible
//!   even when another window is focused.
//! - `_NET_ACTIVE_WINDOW` with pager source indication requests focus, which
//!   bypasses GNOME's focus-stealing prevention (the popup would otherwise open
//!   unfocused, behind the active window).

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConnectionExt, EventMask, PropMode,
};
use x11rb::wrapper::ConnectionExt as _;

/// Set the static window properties. Call before the window is mapped so the
/// compositor reads them at map time.
pub fn apply_static_hints(xid: u32) {
    let Ok((conn, _)) = x11rb::connect(None) else {
        return;
    };
    let atom = |name: &[u8]| atom(&conn, name);
    let (
        Some(wm_type),
        Some(utility),
        Some(wm_state),
        Some(skip_taskbar),
        Some(skip_pager),
        Some(above),
    ) = (
        atom(b"_NET_WM_WINDOW_TYPE"),
        atom(b"_NET_WM_WINDOW_TYPE_UTILITY"),
        atom(b"_NET_WM_STATE"),
        atom(b"_NET_WM_STATE_SKIP_TASKBAR"),
        atom(b"_NET_WM_STATE_SKIP_PAGER"),
        atom(b"_NET_WM_STATE_ABOVE"),
    ) else {
        return;
    };

    let atom_type: u32 = AtomEnum::ATOM.into();
    let _ = conn.change_property32(PropMode::REPLACE, xid, wm_type, atom_type, &[utility]);
    let _ = conn.change_property32(
        PropMode::REPLACE,
        xid,
        wm_state,
        atom_type,
        &[skip_taskbar, skip_pager, above],
    );
    let _ = conn.flush();
}

/// Ask the window manager to focus and raise the window. Call after it is
/// mapped. Source indication 2 ("pager") marks this as a direct user request,
/// which Mutter honours despite focus-stealing prevention.
pub fn activate(xid: u32) {
    let Ok((conn, screen_num)) = x11rb::connect(None) else {
        return;
    };
    let root = conn.setup().roots[screen_num].root;
    let Some(active) = atom(&conn, b"_NET_ACTIVE_WINDOW") else {
        return;
    };

    let data = [2u32, 0, 0, 0, 0];
    let event = ClientMessageEvent::new(32, xid, active, data);
    let _ = conn.send_event(
        false,
        root,
        EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT,
        event,
    );
    let _ = conn.flush();
}

fn atom<C: Connection>(conn: &C, name: &[u8]) -> Option<u32> {
    conn.intern_atom(false, name).ok()?.reply().ok().map(|r| r.atom)
}

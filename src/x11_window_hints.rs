//! XWayland window hints that make the popup behave like a global-hotkey
//! launcher on GNOME.
//!
//! Native Wayland windows can't opt out of the taskbar or force themselves to
//! the front, so Cliccy renders under XWayland (`GDK_BACKEND=x11`) and sets the
//! EWMH properties Mutter honours:
//!
//! - `_NET_WM_WINDOW_TYPE_NORMAL` so Mutter maps and focuses it like any app
//!   window. A `UTILITY` + `SKIP_TASKBAR` window is treated as an auxiliary of a
//!   main window the popup doesn't have, so Mutter gives it second-class
//!   map/focus and it sometimes never surfaces — at the cost of a dock entry
//!   while the popup is open, a normal window appears reliably every time.
//! - `_NET_WM_STATE_ABOVE` keeps it on top so a hotkey-spawned popup is visible
//!   even when another window is focused.
//! - `_NET_ACTIVE_WINDOW` with pager source indication requests focus, which
//!   bypasses GNOME's focus-stealing prevention (the popup would otherwise open
//!   unfocused, behind the active window).

use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConfigureWindowAux, ConnectionExt, EventMask, PropMode,
};
use x11rb::wrapper::ConnectionExt as _;

/// Set the static window properties. Call before the window is mapped so the
/// compositor reads them at map time.
pub fn apply_static_hints(xid: u32) {
    let Ok((conn, _)) = x11rb::connect(None) else {
        return;
    };
    let atom = |name: &[u8]| atom(&conn, name);
    let (Some(wm_type), Some(normal), Some(wm_state), Some(above)) = (
        atom(b"_NET_WM_WINDOW_TYPE"),
        atom(b"_NET_WM_WINDOW_TYPE_NORMAL"),
        atom(b"_NET_WM_STATE"),
        atom(b"_NET_WM_STATE_ABOVE"),
    ) else {
        return;
    };

    let atom_type: u32 = AtomEnum::ATOM.into();
    // A normal, taskbar-visible window so Mutter maps + focuses it reliably (see
    // module docs); keep-above pins it to the top like a launcher.
    let _ = conn.change_property32(PropMode::REPLACE, xid, wm_type, atom_type, &[normal]);
    let _ = conn.change_property32(PropMode::REPLACE, xid, wm_state, atom_type, &[above]);
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

/// Move the window so it is centered on the primary monitor. `win_w`/`win_h` are
/// the window's size in **device** pixels. Call before the window maps to place
/// it without a visible jump. No-op on the pure-Wayland backend (no X connection
/// / XID).
///
/// Centering a GTK4 window under Mutter needs two cooperating mechanisms:
///
/// - **Pre-map:** `WM_NORMAL_HINTS` with the `USPosition` flag, so Mutter places
///   the window centered at first map instead of applying its cascade. GTK also
///   writes this property, so the flag can be raced away — hence the second part.
/// - **Post-map:** a `_NET_MOVERESIZE_WINDOW` client message — the EWMH "move
///   this managed window" request, which Mutter honours regardless of placement
///   policy once the window is mapped. A bare `ConfigureWindow` during the map
///   storm is ignored, so this must be sent after the map settles (see the
///   deferred call in `ui.rs`).
///
/// Calling this both before map (via realize) and after (deferred from map) makes
/// it center with no jump when the hint wins, and reliably centers otherwise.
pub fn center_on_primary(xid: u32, win_w: u32, win_h: u32) {
    let Ok((conn, screen_num)) = x11rb::connect(None) else {
        return;
    };
    let screen = &conn.setup().roots[screen_num];
    let root = screen.root;

    // Prefer the RandR primary monitor; fall back to the whole X screen if no
    // primary output is configured (e.g. single-head or headless XWayland).
    let (mx, my, mw, mh) = primary_geometry(&conn, root).unwrap_or((
        0,
        0,
        screen.width_in_pixels as i32,
        screen.height_in_pixels as i32,
    ));

    let x = mx + (mw - win_w as i32) / 2;
    let y = my + (mh - win_h as i32) / 2;

    set_user_position_hint(&conn, xid, x, y, win_w, win_h);
    if let Some(moveresize) = atom(&conn, b"_NET_MOVERESIZE_WINDOW") {
        // flags: gravity 0 | x present (1<<8) | y present (1<<9) | source pager
        // (2<<12). The WM then moves the managed window to (x, y).
        let flags = (1u32 << 8) | (1 << 9) | (2 << 12);
        let data = [flags, x as u32, y as u32, win_w, win_h];
        let event = ClientMessageEvent::new(32, xid, moveresize, data);
        let _ = conn.send_event(
            false,
            root,
            EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT,
            event,
        );
    }
    // Fallback for WMs without `_NET_MOVERESIZE_WINDOW`; ignored by Mutter mid-map
    // but harmless.
    let _ = conn.configure_window(xid, &ConfigureWindowAux::new().x(x).y(y));
    let _ = conn.flush();
}

/// Write `WM_NORMAL_HINTS` with the `USPosition` flag so Mutter treats our
/// placement as an explicit user request and honours it at map time, instead of
/// applying its own cascade. The obsolete x/y/width/height slots are filled too,
/// since the WM reads the position from there for the USPosition case.
fn set_user_position_hint<C: Connection>(conn: &C, xid: u32, x: i32, y: i32, w: u32, h: u32) {
    const US_POSITION: u32 = 1; // bit 0 of the WM_SIZE_HINTS flags field
    let mut hints = [0u32; 18];
    hints[0] = US_POSITION;
    hints[1] = x as u32;
    hints[2] = y as u32;
    hints[3] = w;
    hints[4] = h;
    let normal_hints: u32 = AtomEnum::WM_NORMAL_HINTS.into();
    let size_hints: u32 = AtomEnum::WM_SIZE_HINTS.into();
    let _ = conn.change_property32(PropMode::REPLACE, xid, normal_hints, size_hints, &hints);
}

/// Ask the WM to add the keep-above state to an already-mapped window. Writing
/// `_NET_WM_STATE` as a property only takes effect before the window is mapped;
/// once Mutter manages it, the state must be toggled via a client message — so
/// this is what actually keeps the popup at the top of the stack. Call after map.
pub fn raise_above(xid: u32) {
    let Ok((conn, screen_num)) = x11rb::connect(None) else {
        return;
    };
    let root = conn.setup().roots[screen_num].root;
    let (Some(wm_state), Some(above)) = (
        atom(&conn, b"_NET_WM_STATE"),
        atom(&conn, b"_NET_WM_STATE_ABOVE"),
    ) else {
        return;
    };

    // data: [action=ADD, first property, second property=0, source=pager, 0]
    let data = [1u32, above, 0, 2, 0];
    let event = ClientMessageEvent::new(32, xid, wm_state, data);
    let _ = conn.send_event(
        false,
        root,
        EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT,
        event,
    );
    let _ = conn.flush();
}

/// Geometry (x, y, width, height) of the RandR primary monitor, in device
/// pixels, or `None` if it can't be resolved.
fn primary_geometry<C: Connection>(conn: &C, root: u32) -> Option<(i32, i32, i32, i32)> {
    let primary = conn.randr_get_output_primary(root).ok()?.reply().ok()?.output;
    if primary == 0 {
        return None;
    }
    let output = conn.randr_get_output_info(primary, 0).ok()?.reply().ok()?;
    if output.crtc == 0 {
        return None;
    }
    let crtc = conn.randr_get_crtc_info(output.crtc, 0).ok()?.reply().ok()?;
    Some((
        crtc.x as i32,
        crtc.y as i32,
        crtc.width as i32,
        crtc.height as i32,
    ))
}

fn atom<C: Connection>(conn: &C, name: &[u8]) -> Option<u32> {
    conn.intern_atom(false, name).ok()?.reply().ok().map(|r| r.atom)
}

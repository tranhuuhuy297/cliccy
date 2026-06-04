//! Popup window construction, list rendering, and show/hide/copy logic.

use gtk::prelude::*;
use gtk::{
    gdk, glib, Application, ApplicationWindow, Box as GtkBox, EventControllerKey, Label, ListBox,
    Orientation, ScrolledWindow, Text,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::app::{AppState, Shared};
use crate::clipboard_backend::ClipContent;
use crate::store::{Entry, Kind, Store};
use crate::{clipboard_backend, config, keys, ui_row};

/// Catppuccin-Mocha theme. Mirrors the reference popup design and, crucially,
/// overrides Adwaita's default widget chrome (entry frame, scrollbar, button,
/// row) on every node so nothing bleeds the system theme through.
const CSS: &str = "
.cliccy { background-color: #1e1e2e; border-radius: 16px;
    border: 1px solid alpha(#6c7086, 0.22);
    font-family: \"Inter\", \"Cantarell\", \"Noto Sans\", sans-serif; }

/* ---- header + flat search box ---- */
.cliccy-header { padding: 12px 14px 11px; }
.cliccy-logo { margin: 0; }
.cliccy-search { padding: 8px 12px; border-radius: 10px;
    background-color: #313244; border: 1px solid transparent; }
.cliccy-search:focus-within { border-color: alpha(#cba6f7, 0.6);
    box-shadow: 0 0 0 3px alpha(#cba6f7, 0.18); }
.cliccy-search-icon { color: #6c7086; }
.cliccy-search-text, .cliccy-search-text:focus {
    background: none; background-color: transparent; color: #cdd6f4;
    font-size: 14px; caret-color: #cba6f7;
    border: none; box-shadow: none; outline: none; padding: 0; min-height: 0; }
.cliccy-search-text selection { background-color: alpha(#cba6f7, 0.35); color: #cdd6f4; }

/* ---- list + overlay scrollbar (reset Adwaita) ---- */
.cliccy scrolledwindow, .cliccy viewport { background: transparent; border: none; }
.cliccy scrollbar { background: transparent; border: none; }
.cliccy scrollbar slider { background-color: alpha(#6c7086, 0.5);
    border-radius: 8px; min-width: 6px; min-height: 6px; }
.cliccy scrollbar slider:hover { background-color: alpha(#6c7086, 0.85); }
.cliccy-list { background: transparent; padding: 4px 8px 8px; }
.cliccy-list row { border-radius: 10px; padding: 0; }
.cliccy-list row:hover:not(:selected) { background-color: alpha(#313244, 0.55); }
.cliccy-list row:selected { background-color: alpha(#585b70, 0.55);
    box-shadow: inset 3px 0 0 #cba6f7; }
.cliccy-row { padding: 9px 11px; }

/* ---- number chip ---- */
.cliccy-num { min-width: 22px; min-height: 19px; color: #6c7086;
    font-family: \"JetBrainsMono Nerd Font\", monospace; font-size: 11px; font-weight: 600;
    background-color: alpha(#6c7086, 0.16); border-radius: 6px; }
.cliccy-list row:selected .cliccy-num { color: #cba6f7;
    background-color: alpha(#cba6f7, 0.16); }

/* ---- kind chip / thumbnail ---- */
.cliccy-kind { min-width: 26px; min-height: 26px; border-radius: 7px;
    background-color: #313244; }
.cliccy-kind.color-chip { background-color: transparent; }
.cliccy-thumb { border-radius: 6px; }

/* ---- body text ---- */
.cliccy-text { color: #cdd6f4; font-size: 13px; }
.cliccy-text.mono { font-family: \"JetBrainsMono Nerd Font\", monospace; font-size: 12px; }
.cliccy-sub { color: #6c7086; font-size: 11px; }
.cliccy-time { color: #6c7086; font-family: \"JetBrainsMono Nerd Font\", monospace;
    font-size: 10px; }

/* ---- hover actions (flat buttons) ---- */
.cliccy-actions { opacity: 0; }
.cliccy-list row:hover .cliccy-actions,
.cliccy-list row:selected .cliccy-actions { opacity: 1; }
.cliccy-act { min-width: 26px; min-height: 26px; padding: 2px; margin: 0;
    border-radius: 7px; background: none; background-color: transparent;
    border: none; box-shadow: none; outline: none; color: #6c7086; }
.cliccy-act:hover { background-color: alpha(#45475a, 0.7); }
.cliccy-act.danger:hover { background-color: alpha(#f38ba8, 0.18); }

/* ---- group headers ---- */
.cliccy-group { padding: 9px 8px 4px 4px; }
.cliccy-group-label { color: #6c7086;
    font-family: \"JetBrainsMono Nerd Font\", monospace; font-size: 10px; font-weight: 700; }
.cliccy-group-line { min-height: 1px; background-color: alpha(#6c7086, 0.22); }

/* ---- footer ---- */
.cliccy-foot { padding: 9px 14px; border-top: 1px solid alpha(#6c7086, 0.22);
    background-color: alpha(#181825, 0.6); }
.cliccy-foot-desc { color: #6c7086; font-size: 11px; }
.cliccy-kbd { min-width: 14px; color: #a6adc8;
    font-family: \"JetBrainsMono Nerd Font\", monospace; font-size: 10px; font-weight: 600;
    background-color: #313244; padding: 1px 6px; border-radius: 5px;
    border: 1px solid alpha(#6c7086, 0.22); }
.cliccy-brand { color: #6c7086;
    font-family: \"JetBrainsMono Nerd Font\", monospace; font-size: 10px; }
";

/// Build the popup window plus shared state, wire events, and return it.
pub fn build(app: &Application) -> Shared {
    let store = Store::open(&config::db_path()).expect("open history database");
    let display = gdk::Display::default().expect("no display available");
    let backend = clipboard_backend::detect();

    // Use the installed themed icon (name == APP_ID) for the window / app logo.
    gtk::Window::set_default_icon_name(config::APP_ID);

    // A pure GTK4 app (no libadwaita) doesn't follow the desktop's dark
    // preference, so its default widgets render in the light Adwaita variant and
    // clash with the popup's dark surface. Force the dark variant so the search
    // field, scrollbar, and buttons share the same palette as the custom CSS.
    if let Some(settings) = gtk::Settings::default() {
        settings.set_property("gtk-application-prefer-dark-theme", true);
    }

    let window = ApplicationWindow::builder()
        .application(app)
        .title("Cliccy")
        .icon_name(config::APP_ID)
        .default_width(640)
        .default_height(440)
        .resizable(false)
        .decorated(false)
        .build();
    window.add_css_class("cliccy");

    let vbox = GtkBox::new(Orientation::Vertical, 0);

    // Header: the logo mark, then a flat search box (magnifier glyph + bare text
    // widget). Using `Text` rather than `SearchEntry` avoids Adwaita's entry
    // frame so the field matches the design's pill.
    let header = GtkBox::new(Orientation::Horizontal, 10);
    header.add_css_class("cliccy-header");
    if let Some(logo) = load_logo() {
        logo.add_css_class("cliccy-logo");
        header.append(&logo);
    }

    let search_box = GtkBox::new(Orientation::Horizontal, 9);
    search_box.add_css_class("cliccy-search");
    search_box.set_hexpand(true);
    if let Some(icon) = ui_row::search_glyph() {
        icon.add_css_class("cliccy-search-icon");
        search_box.append(&icon);
    }
    let search = Text::new();
    search.set_hexpand(true);
    search.set_placeholder_text(Some("Search clipboard history…"));
    search.add_css_class("cliccy-search-text");
    search_box.append(&search);
    header.append(&search_box);
    vbox.append(&header);

    let list = ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.add_css_class("cliccy-list");
    let scroller = ScrolledWindow::builder().vexpand(true).child(&list).build();
    vbox.append(&scroller);

    // Footer hint bar: documents the shortcuts and ties the row numbers to Alt.
    vbox.append(&build_footer());

    window.set_child(Some(&vbox));

    let provider = gtk::CssProvider::new();
    provider.load_from_data(CSS);
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let state: Shared = Rc::new(AppState {
        store,
        window: window.clone(),
        search: search.clone(),
        list: list.clone(),
        scroller: scroller.clone(),
        backend,
        current: RefCell::new(Vec::new()),
        last_seen: RefCell::new(None),
        hold: RefCell::new(None),
        suppress_focus_hide: Cell::new(false),
        last_show: Cell::new(None),
        tray_tx: RefCell::new(None),
    });

    wire_events(&state);
    state
}

fn wire_events(state: &Shared) {
    // Live-filter as the user types.
    let s = state.clone();
    state.search.connect_changed(move |_| refresh(&s));

    // Single click / row activation copies that entry.
    let s = state.clone();
    state.list.connect_row_activated(move |_, row| {
        let entry = s.current.borrow().get(row.index() as usize).cloned();
        if let Some(entry) = entry {
            copy_entry(&s, &entry);
        }
    });

    // Attach "Pinned" / "Recent" section headers above the first row of each
    // group. Headers live above rows (not as their own rows), so the row index
    // still maps 1:1 to `current` for keyboard navigation. Suppressed while
    // searching, where the list is a flat ranked result.
    let s = state.clone();
    state.list.set_header_func(move |row, _before| {
        let idx = row.index();
        if idx < 0 || !s.search.text().is_empty() {
            row.set_header(gtk::Widget::NONE);
            return;
        }
        let cur = s.current.borrow();
        let Some(entry) = cur.get(idx as usize) else {
            row.set_header(gtk::Widget::NONE);
            return;
        };
        let header = if idx == 0 {
            Some(entry.pinned)
        } else if cur.get((idx - 1) as usize).is_some_and(|p| p.pinned) && !entry.pinned {
            Some(false)
        } else {
            None
        };
        match header {
            Some(pinned) => row.set_header(Some(&ui_row::group_header(pinned))),
            None => row.set_header(gtk::Widget::NONE),
        }
    });

    // Keyboard navigation, captured before the search entry consumes the keys.
    let key = EventControllerKey::new();
    key.set_propagation_phase(gtk::PropagationPhase::Capture);
    let s = state.clone();
    key.connect_key_pressed(move |_, keyval, _code, modifier| keys::handle(&s, keyval, modifier));
    state.window.add_controller(key);

    // Closing the window (e.g. window-manager close) just hides the daemon.
    let s = state.clone();
    state.window.connect_close_request(move |_| {
        hide(&s);
        glib::Propagation::Stop
    });

    // Under XWayland, set static EWMH hints before the first map (skip-taskbar,
    // keep-above) so it stays out of the dock and shows on top. No-op on the
    // Wayland backend (downcast fails).
    state.window.connect_realize(|w| {
        if let Some(xid) = x11_xid(w) {
            crate::x11_window_hints::apply_static_hints(xid);
            // Center before the first map so the popup appears centered with no
            // visible jump.
            if let Some((dw, dh)) = device_size(w) {
                crate::x11_window_hints::center_on_primary(xid, dw, dh);
            }
        }
    });

    // Each time it maps, ask the WM to focus/raise it — a hotkey-spawned popup
    // is otherwise left unfocused behind the active window by focus-steal
    // prevention, so the user never sees it. Re-assert centering here too, in
    // case the WM placed the window itself on (re)map.
    let s = state.clone();
    state.window.connect_map(move |w| {
        // Re-assert front-most + focused, with retries: a single client message
        // often reaches Mutter before the XWayland surface finishes mapping and
        // is dropped, leaving the popup behind the active window (the cause of
        // "press the hotkey several times before it appears"). See request_front.
        request_front(&s);
        // Center *after* the map settles, with retries. Mutter ignores a move
        // issued mid-map (it applies its own cascade placement); a single deferred
        // move still loses the race on a *cold* first map, leaving the popup at the
        // top-left until a later map. Re-issuing on a short schedule lands the move
        // once the surface has settled. Each attempt is idempotent — a no-op once
        // the window is already centered.
        center_with_retries(w);
    });

    // Auto-hide when focus genuinely leaves (click elsewhere), like
    // Maccy/Spotlight — but never hide mid-show. The hide is armed *by focus*,
    // not by a timer: `suppress_focus_hide` is set in `show` and cleared the
    // first time the popup actually gains focus. So a popup the WM opens unfocused
    // (focus-steal prevention often ignores our activate request when the open
    // came from the tray-menu grab) stays visible instead of vanishing — the user
    // can use it, Escape, or toggle. Once it has held focus, a focus-out is
    // debounced: the WM toggles active off for a beat while raising/lowering, so
    // we re-check after a short delay and only hide if focus is still gone.
    let s = state.clone();
    state.window.connect_is_active_notify(move |w| {
        if w.is_active() {
            // Settled with focus → arm the auto-hide for subsequent focus-out.
            s.suppress_focus_hide.set(false);
            return;
        }
        if !w.is_visible() || s.suppress_focus_hide.get() {
            return;
        }
        let s = s.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(150), move || {
            if !s.window.is_active() && s.window.is_visible() && !s.suppress_focus_hide.get() {
                hide(&s);
            }
        });
    });
}

/// Center the window now and again on a short retry schedule, so a cold first map
/// (where Mutter cascades the window and drops an early move) still ends centered.
/// Idempotent: once the window sits at the centered position, re-centering is a
/// no-op move.
fn center_with_retries(window: &ApplicationWindow) {
    fn once(window: &ApplicationWindow) {
        if let Some(xid) = x11_xid(window) {
            if let Some((dw, dh)) = device_size(window) {
                crate::x11_window_hints::center_on_primary(xid, dw, dh);
            }
        }
    }
    once(window);
    for delay in [16u64, 60, 140, 280, 450] {
        let w = window.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(delay), move || once(&w));
    }
}

/// The XWayland window XID for a realized window, or `None` on the pure-Wayland
/// backend (where the surface isn't an `X11Surface`).
fn x11_xid(window: &ApplicationWindow) -> Option<u32> {
    let surface = window.surface()?;
    surface.downcast::<gdk4_x11::X11Surface>().ok().map(|x| x.xid() as u32)
}

/// The window's size in device pixels (logical size × scale factor), used to
/// compute the centered position. Falls back to the allocated size if the
/// default size isn't set. `None` if no positive size is known yet.
///
/// Assumes GTK's integer `scale_factor` matches the XWayland surface's device
/// scale; under fractional scaling the two can disagree and shift the popup by
/// half the rounding error. Fine for the common 1×/2× cases.
fn device_size(window: &ApplicationWindow) -> Option<(u32, u32)> {
    let scale = window.scale_factor().max(1);
    let width = window.default_width().max(window.width());
    let height = window.default_height().max(window.height());
    if width <= 0 || height <= 0 {
        return None;
    }
    Some(((width * scale) as u32, (height * scale) as u32))
}

/// Rebuild the visible list from the database, applying the current search filter.
pub fn refresh(state: &Shared) {
    let query = state.search.text().to_lowercase();
    let entries: Vec<Entry> = state
        .store
        .list()
        .unwrap_or_default()
        .into_iter()
        // Text entries match by substring; images only show when not searching.
        .filter(|e| {
            query.is_empty()
                || e.text
                    .as_deref()
                    .is_some_and(|t| t.to_lowercase().contains(&query))
        })
        .collect();

    while let Some(child) = state.list.first_child() {
        state.list.remove(&child);
    }
    for (i, entry) in entries.iter().enumerate() {
        state.list.append(&ui_row::make_row(state, entry, i));
    }
    *state.current.borrow_mut() = entries;
    // Recompute section headers now that `current` reflects the new rows.
    state.list.invalidate_headers();

    if let Some(first) = state.list.row_at_index(0) {
        state.list.select_row(Some(&first));
    }
    // Selecting row 0 doesn't move the viewport, so a scroll position left over
    // from a previous show (or pre-filter) would keep the first row off-screen.
    // Snap the scroller back to the top to match the row-0 selection.
    state.scroller.vadjustment().set_value(0.0);
}

/// Single chokepoint to call after any history mutation: rebuild the popup list
/// if it's currently showing (hidden popups re-`refresh` on next `show`), and
/// always re-push the tray menu's quick picks so they reflect the new state.
pub fn notify_change(state: &Shared) {
    if state.window.is_visible() {
        refresh(state);
    }
    crate::tray::push_menu(state);
}

/// Build the footer hint bar — styled `kbd` chips for each shortcut plus the
/// brand mark, mirroring the popup design.
fn build_footer() -> GtkBox {
    let foot = GtkBox::new(Orientation::Horizontal, 14);
    foot.add_css_class("cliccy-foot");

    let hints: [(&[&str], &str); 5] = [
        (&["↵"], "copy"),
        (&["Alt", "1–9"], "pick"),
        (&["Ctrl", "P"], "pin"),
        (&["Del"], "remove"),
        (&["Esc"], "close"),
    ];
    for (keys, desc) in hints {
        let hint = GtkBox::new(Orientation::Horizontal, 4);
        for k in keys {
            let kbd = Label::new(Some(k));
            kbd.add_css_class("cliccy-kbd");
            hint.append(&kbd);
        }
        let d = Label::new(Some(desc));
        d.add_css_class("cliccy-foot-desc");
        hint.append(&d);
        foot.append(&hint);
    }

    let spacer = GtkBox::new(Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    foot.append(&spacer);

    let brand = Label::new(Some("cliccy"));
    brand.add_css_class("cliccy-brand");
    foot.append(&brand);
    foot
}

/// Decode the bundled SVG logo into a small `Image`. The SVG is compiled into
/// the binary, so this works for both installed and `cargo run` builds. Returns
/// `None` if the platform's gdk-pixbuf SVG loader is unavailable, in which case
/// the header simply renders without a logo.
fn load_logo() -> Option<gtk::Image> {
    const LOGO_SVG: &[u8] = include_bytes!("../assets/com.cliccy.Cliccy.svg");
    const LOGO_PX: i32 = 24;
    // Render 4× and downsample so the gradient glyph stays crisp (and HiDPI-ready).
    let bytes = glib::Bytes::from_static(LOGO_SVG);
    let stream = gtk::gio::MemoryInputStream::from_bytes(&bytes);
    let pixbuf = gtk::gdk_pixbuf::Pixbuf::from_stream_at_scale(
        &stream,
        LOGO_PX * 4,
        LOGO_PX * 4,
        true,
        gtk::gio::Cancellable::NONE,
    )
    .ok()?;
    let texture = gdk::Texture::for_pixbuf(&pixbuf);
    let image = gtk::Image::from_paintable(Some(&texture));
    image.set_pixel_size(LOGO_PX);
    image.set_halign(gtk::Align::Center);
    image.set_valign(gtk::Align::Center);
    Some(image)
}

/// Copy an entry back to the clipboard, bump it to the top, hide, and refresh the
/// tray quick picks (the copied row just became newest).
pub fn copy_entry(state: &Shared, entry: &Entry) {
    let content = match entry.kind {
        Kind::Text => ClipContent::Text(entry.text.clone().unwrap_or_default()),
        Kind::Image => ClipContent::Image(entry.image.clone().unwrap_or_default()),
    };
    state.backend.write(&content);
    // Pre-seed last_seen so the watcher doesn't re-record our own write.
    *state.last_seen.borrow_mut() = Some(clipboard_backend::dedup_key(&content));
    match entry.kind {
        Kind::Text => {
            if let Some(t) = &entry.text {
                let _ = state.store.record_text(t);
            }
        }
        Kind::Image => {
            if let Some(b) = &entry.image {
                let _ = state.store.record_image(b);
            }
        }
    }
    hide(state);
    // Popup is now hidden, so this only re-pushes the tray menu (no list rebuild).
    notify_change(state);
}

/// Ask the WM to focus + raise the popup, then re-send the request a few times
/// over ~300ms. A single `_NET_ACTIVE_WINDOW` / `_NET_WM_STATE_ABOVE` client
/// message frequently races the XWayland surface map and is dropped by Mutter,
/// so the popup opens unfocused behind the active window and the user has to
/// press the hotkey repeatedly. Re-sending on a short schedule lands the request
/// once the surface is actually mapped. Each retry stops early once the window
/// is focused; the messages are idempotent while it isn't. No-op under Wayland
/// (no XID).
fn request_front(state: &Shared) {
    fn send(state: &Shared) {
        if let Some(xid) = x11_xid(&state.window) {
            crate::x11_window_hints::activate(xid);
            crate::x11_window_hints::raise_above(xid);
        }
    }
    send(state);
    for delay in [50u64, 120, 220, 360, 550] {
        let s = state.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(delay), move || {
            // Keep retrying only while it's still up but stuck behind/unfocused.
            if s.window.is_visible() && !s.window.is_active() {
                send(&s);
            }
        });
    }
}

/// Opt-in diagnostic log (set `CLICCY_LOG=1`), written to the daemon's stderr.
/// Used to trace show/hide/toggle sequencing when the popup misbehaves.
fn log(msg: &str) {
    if std::env::var_os("CLICCY_LOG").is_some() {
        eprintln!("[cliccy] {msg}");
    }
}

pub fn show(state: &Shared) {
    // Suppress focus-out auto-hide until the popup actually gains focus. The WM
    // emits a transient active→inactive flicker while raising the popup, and the
    // tray-menu grab releases focus on an unpredictable delay; suppressing until
    // first focus (rather than for a fixed grace period) means neither can hide
    // the popup the instant it appears ("sometimes doesn't show"). The is-active
    // watcher clears this flag on the first real focus and handles genuine
    // focus-out from then on.
    state.suppress_focus_hide.set(true);
    // Arm the double-fire guard: a near-simultaneous second toggle that would hide
    // this popup is ignored for HIDE_GUARD (see `toggle`).
    state.last_show.set(Some(std::time::Instant::now()));
    state.search.set_text("");
    refresh(state);
    state.window.present();
    state.search.grab_focus();

    // Force the popup to the front and re-request focus on every show. `present()`
    // re-emits `map` (where `connect_map` raises/activates) only on a real
    // hidden→shown transition; when the window is already mapped but stuck behind
    // the active window unfocused (GNOME focus-steal prevention), no map fires, so
    // it would never be re-raised. Re-assert here too — these are idempotent EWMH
    // client messages, harmless to send when already on top. No-op under Wayland.
    request_front(state);

    // Fallback clear so `suppress_focus_hide` can never stick `true` forever when
    // the WM never grants focus (popup opened behind). The auto-hide is
    // edge-triggered on focus-out, so re-arming it late only matters for a
    // *future* focus-out — it can't vanish a stable unfocused popup.
    let s = state.clone();
    glib::timeout_add_local_once(std::time::Duration::from_millis(600), move || {
        s.suppress_focus_hide.set(false);
    });
}

/// Re-assert the popup to the front without resetting its contents. Used by the
/// tray "Open" path: the GNOME menu grab can swallow the initial `present()`, so
/// a second present once the grab has released brings the popup up reliably.
/// Unlike `show`, this preserves the search text and selection (the user may have
/// started typing), and it is a near no-op when the window is already up front.
pub fn represent(state: &Shared) {
    log("represent (tray re-present after menu grab)");
    state.window.present();
    request_front(state);
}

pub fn hide(state: &Shared) {
    state.window.set_visible(false);
}

/// A toggle resolving to "hide" within this window of the last show is treated as
/// a double-fire and ignored, so two racing `cliccy toggle` processes (or a tray
/// double-activate) can't cancel a show into nothing. Long enough to cover the
/// variable process-spawn/forward latency between the two; short enough that a
/// deliberate hide (always well after the popup appeared) is never blocked.
const HIDE_GUARD: std::time::Duration = std::time::Duration::from_millis(350);

pub fn toggle(state: &Shared) {
    // Hide only when the popup is genuinely up-front *and* focused. A popup the WM
    // opened behind the active window (focus-steal prevention, common from the
    // tray-menu grab) reports `is_visible() == true` yet the user never saw it;
    // treating that as "shown" made the next hotkey/Open press hide it, so nothing
    // appeared. When visible-but-not-active, re-`show()` instead — that re-raises
    // and re-focuses it, pulling the stuck popup to the front.
    let (vis, act) = (state.window.is_visible(), state.window.is_active());
    let since = state.last_show.get().map(|t| t.elapsed().as_millis());
    if vis && act {
        // Suppress a hide that lands right after a show (a double-fire) — it would
        // otherwise blank the popup the user just asked for. Never blocks a show.
        if state.last_show.get().is_some_and(|t| t.elapsed() < HIDE_GUARD) {
            log(&format!("toggle: vis={vis} act={act} since_show={since:?}ms -> SKIP hide (guard)"));
            return;
        }
        log(&format!("toggle: vis={vis} act={act} since_show={since:?}ms -> hide"));
        hide(state);
    } else {
        log(&format!("toggle: vis={vis} act={act} since_show={since:?}ms -> show"));
        show(state);
    }
}

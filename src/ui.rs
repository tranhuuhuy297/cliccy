//! Popup window construction, list rendering, and show/hide/copy logic.

use gtk::prelude::*;
use gtk::{
    gdk, glib, Application, ApplicationWindow, Box as GtkBox, EventControllerKey, Label, ListBox,
    ListBoxRow, Orientation, ScrolledWindow, SearchEntry,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::app::{AppState, Shared};
use crate::store::{Entry, Store};
use crate::{clipboard_backend, config, keys};

/// Catppuccin-Mocha flavoured dark theme, echoing Maccy's compact popup look.
const CSS: &str = "
.cliccy { background-color: #1e1e2e; border-radius: 12px; }
.cliccy-logo { margin-left: 16px; }
.cliccy-search { margin: 10px; padding: 8px 12px; border-radius: 8px;
    background-color: #313244; color: #cdd6f4; font-size: 14px; }
.cliccy-list { background: transparent; }
.cliccy-list row:selected { background-color: #585b70; }
.cliccy-row { padding: 8px 14px; }
.cliccy-num { color: #6c7086; min-width: 18px; font-size: 12px; }
.cliccy-row label { color: #cdd6f4; }
";

const PREVIEW_CHARS: usize = 120;

/// Build the popup window plus shared state, wire events, and return it.
pub fn build(app: &Application) -> Shared {
    let store = Store::open(&config::db_path()).expect("open history database");
    let display = gdk::Display::default().expect("no display available");
    let backend = clipboard_backend::detect();

    // Use the installed themed icon (name == APP_ID) for the window / app logo.
    gtk::Window::set_default_icon_name(config::APP_ID);

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

    // Header row: the logo mark sits left of the search field. The logo's tile
    // shares the popup background colour, so only its gradient glyph shows here.
    let header = GtkBox::new(Orientation::Horizontal, 8);
    header.add_css_class("cliccy-header");
    if let Some(logo) = load_logo() {
        logo.add_css_class("cliccy-logo");
        header.append(&logo);
    }

    let search = SearchEntry::new();
    search.set_placeholder_text(Some("Search clipboard history…"));
    search.set_hexpand(true);
    search.add_css_class("cliccy-search");
    header.append(&search);
    vbox.append(&header);

    let list = ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.add_css_class("cliccy-list");
    let scroller = ScrolledWindow::builder().vexpand(true).child(&list).build();
    vbox.append(&scroller);

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
        backend,
        current: RefCell::new(Vec::new()),
        last_seen: RefCell::new(None),
        hold: RefCell::new(None),
        suppress_focus_hide: Cell::new(false),
    });

    wire_events(&state);
    state
}

fn wire_events(state: &Shared) {
    // Live-filter as the user types.
    let s = state.clone();
    state.search.connect_search_changed(move |_| refresh(&s));

    // Single click / row activation copies that entry.
    let s = state.clone();
    state.list.connect_row_activated(move |_, row| {
        let content = s
            .current
            .borrow()
            .get(row.index() as usize)
            .map(|e| e.content.clone());
        if let Some(c) = content {
            copy(&s, &c);
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
        }
    });

    // Each time it maps, ask the WM to focus/raise it — a hotkey-spawned popup
    // is otherwise left unfocused behind the active window by focus-steal
    // prevention, so the user never sees it.
    state.window.connect_map(|w| {
        if let Some(xid) = x11_xid(w) {
            crate::x11_window_hints::activate(xid);
        }
    });

    // Auto-hide when focus leaves (click elsewhere), like Maccy/Spotlight. This
    // keeps `is_visible` in sync with what the user sees, so the hotkey toggle
    // doesn't get stuck "hiding" an already-dismissed popup. `suppress_focus_hide`
    // covers the brief unfocused moment during the show/raise transition.
    let s = state.clone();
    state.window.connect_is_active_notify(move |w| {
        if w.is_active() {
            s.suppress_focus_hide.set(false);
        } else if w.is_visible() && !s.suppress_focus_hide.get() {
            hide(&s);
        }
    });
}

/// The XWayland window XID for a realized window, or `None` on the pure-Wayland
/// backend (where the surface isn't an `X11Surface`).
fn x11_xid(window: &ApplicationWindow) -> Option<u32> {
    let surface = window.surface()?;
    surface.downcast::<gdk4_x11::X11Surface>().ok().map(|x| x.xid() as u32)
}

/// Rebuild the visible list from the database, applying the current search filter.
pub fn refresh(state: &Shared) {
    let query = state.search.text().to_lowercase();
    let entries: Vec<Entry> = state
        .store
        .list()
        .unwrap_or_default()
        .into_iter()
        .filter(|e| query.is_empty() || e.content.to_lowercase().contains(&query))
        .collect();

    while let Some(child) = state.list.first_child() {
        state.list.remove(&child);
    }
    for (i, entry) in entries.iter().enumerate() {
        state.list.append(&make_row(entry, i));
    }
    *state.current.borrow_mut() = entries;

    if let Some(first) = state.list.row_at_index(0) {
        state.list.select_row(Some(&first));
    }
}

/// Decode the bundled SVG logo into a small `Image`. The SVG is compiled into
/// the binary, so this works for both installed and `cargo run` builds. Returns
/// `None` if the platform's gdk-pixbuf SVG loader is unavailable, in which case
/// the header simply renders without a logo.
fn load_logo() -> Option<gtk::Image> {
    const LOGO_SVG: &[u8] = include_bytes!("../assets/com.cliccy.Cliccy.svg");
    let bytes = glib::Bytes::from_static(LOGO_SVG);
    let stream = gtk::gio::MemoryInputStream::from_bytes(&bytes);
    let pixbuf = gtk::gdk_pixbuf::Pixbuf::from_stream_at_scale(
        &stream,
        22,
        22,
        true,
        gtk::gio::Cancellable::NONE,
    )
    .ok()?;
    let texture = gdk::Texture::for_pixbuf(&pixbuf);
    let image = gtk::Image::from_paintable(Some(&texture));
    image.set_pixel_size(22);
    Some(image)
}

fn make_row(entry: &Entry, index: usize) -> ListBoxRow {
    let row = ListBoxRow::new();
    let hbox = GtkBox::new(Orientation::Horizontal, 8);
    hbox.add_css_class("cliccy-row");

    let badge = if index < 9 {
        (index + 1).to_string()
    } else {
        String::new()
    };
    let num = Label::new(Some(&badge));
    num.add_css_class("cliccy-num");
    hbox.append(&num);

    if entry.pinned {
        hbox.append(&Label::new(Some("📌")));
    }

    let preview: String = entry.content.replace('\n', " ").chars().take(PREVIEW_CHARS).collect();
    let label = Label::new(Some(&preview));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    hbox.append(&label);

    row.set_child(Some(&hbox));
    row
}

/// Copy `content` back to the system clipboard, bump it to the top, and hide.
pub fn copy(state: &Shared, content: &str) {
    state.backend.write(content);
    // Pre-seed last_seen so the poller doesn't re-record our own write.
    *state.last_seen.borrow_mut() = Some(content.to_string());
    let _ = state.store.record(content);
    hide(state);
}

pub fn show(state: &Shared) {
    // Suppress focus-out auto-hide until the popup has actually gained focus.
    state.suppress_focus_hide.set(true);
    state.search.set_text("");
    refresh(state);
    state.window.present();
    state.search.grab_focus();
}

pub fn hide(state: &Shared) {
    state.window.set_visible(false);
}

pub fn toggle(state: &Shared) {
    if state.window.is_visible() {
        hide(state);
    } else {
        show(state);
    }
}

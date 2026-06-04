//! System-tray (StatusNotifierItem) icon for the GNOME top bar.
//!
//! The popup itself is deliberately kept out of the dock/taskbar (see
//! `x11_window_hints`), so this tray icon is the only persistent, clickable
//! surface the resident daemon exposes. Left-click toggles the popup; the
//! context menu offers open / clear-history / quit.
//!
//! `ksni` speaks the StatusNotifierItem D-Bus protocol over pure-Rust `zbus`
//! (no libdbus/libappindicator C dependency). Ubuntu 22.04+ ships the
//! AppIndicator GNOME extension enabled by default, so the icon appears in the
//! top bar with no extra setup.
//!
//! Threading: ksni serves D-Bus on its own thread, so its callbacks run off the
//! glib main thread and must not touch the single-threaded `AppState`. They
//! forward a `TrayCommand` over an `async_channel`; a `glib` task on the main
//! thread receives it and drives the UI.

use gtk::gdk_pixbuf::prelude::*;
use gtk::gdk_pixbuf::PixbufLoader;
use gtk::prelude::ApplicationExt;
use gtk::{glib, Application};
use ksni::menu::StandardItem;
use ksni::{Icon, MenuItem, Tray, TrayMethods};

use crate::app::Shared;
use crate::store::{Entry, Kind, Store};
use crate::{config, ui};

/// White, transparent scissors glyph rendered into the binary, so the tray icon
/// needs no installed theme file and shows correctly on the dark top bar.
const TRAY_ICON_PNG: &[u8] = include_bytes!("../assets/cliccy-tray-white.png");

/// How many pinned and how many recent entries to surface as quick-pick items in
/// the tray menu, so the menu stays a glance rather than the full history.
const QUICK_PICKS_PER_SECTION: usize = 3;

/// Max characters of a text entry shown as its menu label before ellipsis.
const LABEL_MAX_CHARS: usize = 48;

/// A single quick-pick row mirrored into the tray menu. Carries only `Send` data
/// (a display label + the row id to copy on click), so the snapshot can cross from
/// the glib main thread to ksni's D-Bus thread.
#[derive(Clone)]
pub struct TrayEntry {
    /// History row id, sent back as `TrayCommand::Copy(id)` when the item is clicked.
    pub id: i64,
    /// Pre-formatted, escaped label as it should appear in the menu.
    pub label: String,
    /// Drives which section ("Pinned" / "Recent") the item renders under.
    pub pinned: bool,
}

/// Commands forwarded from the tray's D-Bus thread to the glib main thread.
#[derive(Clone)]
enum TrayCommand {
    /// Left-click on the icon: show if hidden, hide if shown.
    Toggle,
    /// The "Open Cliccy" menu item: always show, never hide. A toggle here is
    /// wrong twice over — the label says "Open", and GTK's `is_active()` can read
    /// stale-true on the hidden window, making a toggle resolve to "hide".
    Show,
    /// A quick-pick item was clicked: copy the history row with this id.
    Copy(i64),
    Clear,
    Quit,
}

/// The StatusNotifierItem. Holds only `Send` data (the channel sender, the
/// pre-decoded icon pixels, and the current quick-pick snapshot), so it can live
/// on ksni's background thread while the real work happens on the glib main thread.
struct CliccyTray {
    tx: async_channel::Sender<TrayCommand>,
    /// Pre-decoded ARGB icon, handed to the host verbatim. Empty if decoding the
    /// embedded PNG failed (the item still works, just without a custom glyph).
    icons: Vec<Icon>,
    /// Current top pinned + top recent entries to list in the menu. Refreshed from
    /// the main thread via `Handle::update` whenever the history changes.
    entries: Vec<TrayEntry>,
}

impl Tray for CliccyTray {
    fn id(&self) -> String {
        config::APP_ID.to_string()
    }

    fn title(&self) -> String {
        "Cliccy".to_string()
    }

    /// Supply the icon as raw pixels rather than a theme name, so it renders
    /// without depending on an installed `.svg` or the icon cache. `icon_name`
    /// is intentionally left empty so the host falls through to this.
    fn icon_pixmap(&self) -> Vec<Icon> {
        self.icons.clone()
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        // `bounded(8)` + `send_blocking`: the glib consumer drains commands
        // cheaply, so the queue only fills if the main loop is wedged — at which
        // point briefly blocking this D-Bus thread is harmless. A send error
        // just means the receiver is gone (app already quitting): nothing to do.
        let _ = self.tx.send_blocking(TrayCommand::Toggle);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let mut items: Vec<MenuItem<Self>> = vec![command_item("Open Cliccy", TrayCommand::Show)];

        // Quick picks: top pinned then top recent, each under a disabled section
        // header. Sections render only when non-empty, so an empty history (or no
        // pins) just omits them rather than showing a dangling header.
        let pinned: Vec<&TrayEntry> = self.entries.iter().filter(|e| e.pinned).collect();
        let recent: Vec<&TrayEntry> = self.entries.iter().filter(|e| !e.pinned).collect();
        let mut sections = false;
        for (title, rows) in [("Pinned", pinned), ("Recent", recent)] {
            if rows.is_empty() {
                continue;
            }
            items.push(MenuItem::Separator);
            items.push(section_header(title));
            for e in rows {
                items.push(quick_pick_item(e));
            }
            sections = true;
        }
        if sections {
            items.push(MenuItem::Separator);
        }

        items.push(command_item("Clear history", TrayCommand::Clear));
        items.push(MenuItem::Separator);
        items.push(command_item("Quit", TrayCommand::Quit));
        items
    }
}

/// A plain menu item whose only job is to forward a fixed `TrayCommand` to the
/// main thread when activated.
fn command_item(label: &str, cmd: TrayCommand) -> MenuItem<CliccyTray> {
    StandardItem {
        label: label.into(),
        activate: Box::new(move |t: &mut CliccyTray| {
            let _ = t.tx.send_blocking(cmd.clone());
        }),
        ..Default::default()
    }
    .into()
}

/// A disabled, non-clickable label that groups the quick-pick rows beneath it.
fn section_header(title: &str) -> MenuItem<CliccyTray> {
    StandardItem {
        label: title.into(),
        enabled: false,
        ..Default::default()
    }
    .into()
}

/// A clickable quick-pick row: copies its history entry on click.
fn quick_pick_item(entry: &TrayEntry) -> MenuItem<CliccyTray> {
    let id = entry.id;
    StandardItem {
        label: entry.label.clone(),
        activate: Box::new(move |t: &mut CliccyTray| {
            let _ = t.tx.send_blocking(TrayCommand::Copy(id));
        }),
        ..Default::default()
    }
    .into()
}

/// Register the tray icon and wire its commands back to the UI. Called once from
/// the daemon's startup, on the glib main thread.
pub fn install(app: &Application, shared: &Shared) {
    let (tx, rx) = async_channel::bounded::<TrayCommand>(8);
    // Unbounded so the main thread never blocks pushing a fresh snapshot, and so a
    // burst of history changes can't deadlock against the tray thread.
    let (snap_tx, snap_rx) = async_channel::unbounded::<Vec<TrayEntry>>();

    // Decode here on the main thread; the resulting pixels are plain `Send` data
    // moved into the tray that lives on ksni's thread.
    spawn_service(
        CliccyTray {
            tx,
            icons: load_icons(),
            entries: Vec::new(),
        },
        snap_rx,
    );

    // Hand the snapshot sender to the rest of the app and seed the menu with the
    // current history, so the quick picks are populated before the first open.
    *shared.tray_tx.borrow_mut() = Some(snap_tx);
    push_menu(shared);

    let app = app.clone();
    let shared = shared.clone();
    glib::spawn_future_local(async move {
        while let Ok(cmd) = rx.recv().await {
            match cmd {
                TrayCommand::Toggle => ui::toggle(&shared),
                TrayCommand::Show => {
                    // Show now for responsiveness, then re-present once after the
                    // GNOME tray-menu grab has released: the first map can be
                    // swallowed mid-grab, leaving nothing on screen. The second
                    // present is idempotent when the first already worked.
                    ui::show(&shared);
                    let s = shared.clone();
                    glib::timeout_add_local_once(std::time::Duration::from_millis(160), move || {
                        ui::represent(&s);
                    });
                }
                TrayCommand::Copy(id) => {
                    // Look the row up fresh (the snapshot only carries id + label),
                    // then copy it back exactly like a popup pick — which bumps it to
                    // newest and re-pushes the menu via `copy_entry` → `notify_change`.
                    // `get` (not `list`) loads just this one row's image data.
                    if let Ok(Some(entry)) = shared.store.get(id) {
                        ui::copy_entry(&shared, &entry);
                    }
                }
                TrayCommand::Clear => {
                    if shared.store.clear().is_ok() {
                        ui::notify_change(&shared);
                    }
                }
                TrayCommand::Quit => {
                    app.quit();
                    break;
                }
            }
        }
    });
}

/// Build the current quick-pick snapshot and push it to the tray menu. A no-op
/// before the tray is installed (or when no StatusNotifier host accepted it, in
/// which case the receiver is gone and the send simply fails).
pub fn push_menu(shared: &Shared) {
    if let Some(tx) = shared.tray_tx.borrow().as_ref() {
        let _ = tx.try_send(snapshot(&shared.store));
    }
}

/// Take the top pinned + top recent entries and format them for the menu.
/// `Store::list` already orders pinned-first then newest-first, so a per-section
/// `take` preserves that ordering.
fn snapshot(store: &Store) -> Vec<TrayEntry> {
    let (pinned, recent): (Vec<Entry>, Vec<Entry>) = store
        .list_lite()
        .unwrap_or_default()
        .into_iter()
        .partition(|e| e.pinned);
    pinned
        .into_iter()
        .take(QUICK_PICKS_PER_SECTION)
        .chain(recent.into_iter().take(QUICK_PICKS_PER_SECTION))
        .map(|e| TrayEntry {
            id: e.id,
            label: menu_label(&e),
            pinned: e.pinned,
        })
        .collect()
}

/// Format one entry as a single-line menu label: images get a placeholder, text is
/// whitespace-collapsed and truncated. Underscores are doubled so ksni renders them
/// literally instead of as mnemonic markers.
fn menu_label(entry: &Entry) -> String {
    let raw = match entry.kind {
        Kind::Image => "🖼  Image".to_string(),
        Kind::Text => {
            let text = entry.text.as_deref().unwrap_or_default();
            let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
            truncate(&collapsed, LABEL_MAX_CHARS)
        }
    };
    raw.replace('_', "__")
}

/// Truncate to at most `max` characters (by `char`, not byte, so multibyte text is
/// never split mid-codepoint), appending an ellipsis when shortened.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

/// Run the ksni D-Bus service on a dedicated thread with its own current-thread
/// tokio runtime, so it never touches the glib main loop. `spawn()` returns a
/// handle but the actual service loop is a detached tokio task; parking on a
/// never-resolving future keeps the runtime alive to poll that task for the
/// process's lifetime. The returned handle is intentionally dropped — teardown
/// happens on process exit, when the D-Bus name vanishes and the watcher drops
/// the icon. If no StatusNotifier host is present, `spawn()` fails and the
/// thread simply exits — the hotkey popup keeps working regardless.
fn spawn_service(tray: CliccyTray, snap_rx: async_channel::Receiver<Vec<TrayEntry>>) {
    let _ = std::thread::Builder::new()
        .name("cliccy-tray".into())
        .spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            rt.block_on(async move {
                // Keep the handle (it was previously dropped): the menu now carries
                // live quick-pick rows, so the main thread pushes a new snapshot
                // through `Handle::update` whenever the history changes. Awaiting the
                // snapshot channel also keeps this runtime alive to poll the detached
                // D-Bus service task, exactly as the old `pending()` park did.
                let Ok(handle) = tray.spawn().await else {
                    return; // No StatusNotifier host: drop snap_rx, sends become no-ops.
                };
                while let Ok(entries) = snap_rx.recv().await {
                    handle.update(|t| t.entries = entries).await;
                }
            });
        });
}

/// Decode the embedded PNG into the single ARGB32 (network byte order) pixmap the
/// StatusNotifierItem spec expects. Returns an empty vec on any failure, leaving
/// the tray icon-less rather than failing to register.
fn load_icons() -> Vec<Icon> {
    let loader = PixbufLoader::new();
    if loader.write(TRAY_ICON_PNG).is_err() || loader.close().is_err() {
        return Vec::new();
    }
    let Some(pixbuf) = loader.pixbuf() else {
        return Vec::new();
    };
    // The spec wants an alpha channel; synthesise an opaque one if the source
    // somehow lacked it (our asset always has alpha, so this is just a guard).
    let pixbuf = if pixbuf.has_alpha() {
        pixbuf
    } else {
        match pixbuf.add_alpha(false, 0, 0, 0) {
            Ok(p) => p,
            Err(_) => return Vec::new(),
        }
    };

    let (width, height) = (pixbuf.width(), pixbuf.height());
    let rowstride = pixbuf.rowstride() as usize;
    let channels = pixbuf.n_channels() as usize;
    let bytes = pixbuf.read_pixel_bytes();
    let pixels: &[u8] = &bytes;

    let mut data = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height as usize {
        for x in 0..width as usize {
            let i = y * rowstride + x * channels;
            let (r, g, b, a) = (pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]);
            // gdk-pixbuf gives RGBA; the spec wants ARGB, network byte order.
            data.extend_from_slice(&[a, r, g, b]);
        }
    }

    vec![Icon {
        width,
        height,
        data,
    }]
}

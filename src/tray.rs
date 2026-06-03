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
use crate::{config, ui};

/// White, transparent scissors glyph rendered into the binary, so the tray icon
/// needs no installed theme file and shows correctly on the dark top bar.
const TRAY_ICON_PNG: &[u8] = include_bytes!("../assets/cliccy-tray-white.png");

/// Commands forwarded from the tray's D-Bus thread to the glib main thread.
enum TrayCommand {
    Toggle,
    Clear,
    Quit,
}

/// The StatusNotifierItem. Holds only `Send` data (the channel sender and the
/// pre-decoded icon pixels), so it can live on ksni's background thread while the
/// real work happens on the glib main thread.
struct CliccyTray {
    tx: async_channel::Sender<TrayCommand>,
    /// Pre-decoded ARGB icon, handed to the host verbatim. Empty if decoding the
    /// embedded PNG failed (the item still works, just without a custom glyph).
    icons: Vec<Icon>,
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
        vec![
            StandardItem {
                label: "Open Cliccy".into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.tx.send_blocking(TrayCommand::Toggle);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Clear history".into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.tx.send_blocking(TrayCommand::Clear);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.tx.send_blocking(TrayCommand::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Register the tray icon and wire its commands back to the UI. Called once from
/// the daemon's startup, on the glib main thread.
pub fn install(app: &Application, shared: &Shared) {
    let (tx, rx) = async_channel::bounded::<TrayCommand>(8);

    // Decode here on the main thread; the resulting pixels are plain `Send` data
    // moved into the tray that lives on ksni's thread.
    spawn_service(CliccyTray {
        tx,
        icons: load_icons(),
    });

    let app = app.clone();
    let shared = shared.clone();
    glib::spawn_future_local(async move {
        while let Ok(cmd) = rx.recv().await {
            match cmd {
                TrayCommand::Toggle => ui::toggle(&shared),
                TrayCommand::Clear => {
                    if shared.store.clear().is_ok() {
                        ui::refresh(&shared);
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

/// Run the ksni D-Bus service on a dedicated thread with its own current-thread
/// tokio runtime, so it never touches the glib main loop. `spawn()` returns a
/// handle but the actual service loop is a detached tokio task; parking on a
/// never-resolving future keeps the runtime alive to poll that task for the
/// process's lifetime. The returned handle is intentionally dropped — teardown
/// happens on process exit, when the D-Bus name vanishes and the watcher drops
/// the icon. If no StatusNotifier host is present, `spawn()` fails and the
/// thread simply exits — the hotkey popup keeps working regardless.
fn spawn_service(tray: CliccyTray) {
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
                if tray.spawn().await.is_ok() {
                    std::future::pending::<()>().await;
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

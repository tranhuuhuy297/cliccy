//! Keep the installed app icon resolvable, re-checked on each daemon startup.
//!
//! GNOME picks a window's dock / Alt-Tab icon by looking up the desktop entry's
//! `Icon=` name (our app id) in the icon theme. Two things can break that lookup
//! even though the installer placed our SVG under `hicolor/scalable/apps`:
//!
//!  1. A stale `hicolor/icon-theme.cache` — an unrelated install or system update
//!     rebuilds it while our SVG is the only newer file, leaving a cache that is
//!     newer than the icon yet does not list it. GTK trusts the cache and the
//!     name fails to resolve.
//!  2. A minimal local `hicolor/index.theme` that declares only fixed-size apps
//!     dirs (some installers ship one to register their PNG icons) and omits
//!     `scalable/apps`. A theme index in `XDG_DATA_HOME` shadows the system one,
//!     so GTK never searches `scalable/apps` and our SVG is invisible.
//!
//! Re-checking on each daemon launch — and acting only when the icon is genuinely
//! unresolvable — heals both without a reinstall: it drops a PNG render of the
//! logo into a fixed-size apps dir (found even by a minimal index) and rebuilds
//! the cache. Best-effort throughout: any failure leaves the popup working, just
//! with the fallback icon until the next successful refresh.

use std::path::{Path, PathBuf};
use std::process::Command;

use gtk::gdk;
use gtk::gdk_pixbuf::{Pixbuf, PixbufLoader};
use gtk::prelude::*;

use crate::config;

/// The same logo the window and tray use, rendered into a fixed-size apps dir as
/// a PNG fallback when the theme cannot resolve the scalable SVG.
const LOGO_SVG: &[u8] = include_bytes!("../assets/com.cliccy.Cliccy.svg");

/// Pixel sizes to materialise the PNG fallback at: one panel/dock size and one
/// large size, both common `Fixed`/`Scalable` entries a minimal index declares.
const FALLBACK_SIZES: [i32; 2] = [48, 256];

/// If the icon theme cannot resolve our app-id icon, repair it: render a PNG copy
/// of the logo into the fixed-size apps dirs and rebuild the icon cache. Does
/// nothing on a healthy theme, so a normal startup pays only one lookup.
pub fn ensure_resolvable() {
    let Some(hicolor) = hicolor_dir() else {
        return;
    };
    // A display is needed to ask the theme whether our name resolves. Without one
    // (should not happen post-`ui::build`) there is nothing reliable to check.
    let Some(display) = gdk::Display::default() else {
        return;
    };
    if gtk::IconTheme::for_display(&display).has_icon(config::APP_ID) {
        return;
    }

    // Unresolvable. Drop PNG copies into fixed-size apps dirs, which even a
    // minimal `index.theme` declares, then rebuild the cache so the lookup (and
    // GNOME Shell, which watches the cache) can find the name.
    for size in FALLBACK_SIZES {
        let _ = write_png(&hicolor, size);
    }
    rebuild_cache(&hicolor);
}

/// Render the embedded logo to `<hicolor>/<size>x<size>/apps/<app id>.png`.
fn write_png(hicolor: &Path, size: i32) -> Result<(), Box<dyn std::error::Error>> {
    let pixbuf = render_logo(size)?;
    let dir = hicolor.join(format!("{size}x{size}/apps"));
    std::fs::create_dir_all(&dir)?;
    let png = dir.join(format!("{}.png", config::APP_ID));
    pixbuf.savev(&png, "png", &[])?;
    Ok(())
}

/// Decode the embedded SVG at a square target size via the gdk-pixbuf SVG loader.
fn render_logo(size: i32) -> Result<Pixbuf, Box<dyn std::error::Error>> {
    let loader = PixbufLoader::with_type("svg")?;
    // Force the rasterised output to the requested square size.
    loader.connect_size_prepared(move |l, _w, _h| l.set_size(size, size));
    loader.write(LOGO_SVG)?;
    loader.close()?;
    loader.pixbuf().ok_or_else(|| "svg produced no pixbuf".into())
}

/// Rebuild the hicolor icon cache. `-f` forces a rebuild even when mtimes look
/// current (the stale-cache trap), `-t` tolerates an existing cache. Prefer the
/// GTK4 tool, fall back to the GTK3 name.
fn rebuild_cache(hicolor: &Path) {
    for tool in ["gtk4-update-icon-cache", "gtk-update-icon-cache"] {
        let rebuilt = Command::new(tool)
            .args(["-f", "-t"])
            .arg(hicolor)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if rebuilt {
            break;
        }
    }
}

/// `~/.local/share/icons/hicolor`, the user icon theme dir the installer targets.
fn hicolor_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("icons/hicolor"))
}

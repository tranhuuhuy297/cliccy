//! Row rendering for the popup list: the number chip, per-type kind glyph
//! (or image thumbnail / colour swatch), text preview, relative timestamp, and
//! the hover-revealed pin/delete actions. Kept separate from `ui.rs` so the
//! window-construction logic there stays readable.

use gtk::prelude::*;
use gtk::{cairo, gdk, glib, Box as GtkBox, Button, DrawingArea, Label, ListBoxRow, Orientation};
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::app::Shared;
use crate::clipboard_backend::image_key;
use crate::store::{Entry, Kind};
use crate::ui_preview;

// Decoded textures are cached by content so the list rebuild that runs on every
// show and every search keystroke reuses rasterised glyphs/thumbnails instead of
// re-parsing the same SVG or re-decoding the same PNG each time. Rasterising via
// gdk-pixbuf (librsvg for icons, the PNG loader for images) is the dominant cost
// of a refresh; the glyph set is tiny and recurs constantly, so the cache turns
// repeat refreshes near-free. Single-threaded GTK, so thread-local is enough.
thread_local! {
    static TEXTURE_CACHE: RefCell<HashMap<String, gdk::Texture>> = RefCell::new(HashMap::new());
}

/// Soft ceiling on cached textures. The live working set is tiny (a handful of
/// glyphs plus at most ~MAX_UNPINNED+pins thumbnails), so this only ever trips
/// after pathological churn over a long-running daemon — at which point a full
/// clear bounds memory and the next refresh re-decodes lazily.
const TEXTURE_CACHE_CAP: usize = 512;

/// Look up a decoded texture by `key`, or decode it via `build` and cache it.
fn cached_texture(key: String, build: impl FnOnce() -> Option<gdk::Texture>) -> Option<gdk::Texture> {
    if let Some(tex) = TEXTURE_CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return Some(tex);
    }
    let tex = build()?;
    TEXTURE_CACHE.with(|c| {
        let mut cache = c.borrow_mut();
        if cache.len() >= TEXTURE_CACHE_CAP {
            cache.clear();
        }
        cache.insert(key, tex.clone());
    });
    Some(tex)
}

const PREVIEW_CHARS: usize = 120;

/// Catppuccin tokens baked into the inline SVG glyphs (pixbuf can't honour
/// `currentColor`, so each icon carries its own stroke/fill colour).
const OVERLAY: &str = "#6c7086";
const MAUVE: &str = "#cba6f7";

/// Build one list row for `entry` at position `index` (0-based, drives the
/// Alt-1–9 number chip). `state` is captured by the pin/delete buttons.
pub fn make_row(state: &Shared, entry: &Entry, index: usize) -> ListBoxRow {
    let row = ListBoxRow::new();
    let hbox = GtkBox::new(Orientation::Horizontal, 11);
    hbox.add_css_class("cliccy-row");

    hbox.append(&number_chip(index));
    if let Some(chip) = leading(entry) {
        hbox.append(&chip);
    }
    hbox.append(&body(entry));

    // Reveal the full content on hover using the same side panel that Space pops
    // up, so mouse and keyboard surface long/clipped rows identically. No-op for
    // rows whose text already fits.
    ui_preview::attach_hover(state, &row, entry);

    let time = Label::new(Some(&time_ago(entry.copied_at)));
    time.add_css_class("cliccy-time");
    time.set_valign(gtk::Align::Center);
    hbox.append(&time);

    hbox.append(&actions(state, entry));

    row.set_child(Some(&hbox));
    row
}

/// The 1-based position chip shown on rows 1–9, which double as Alt-quick-pick
/// targets. Rows past the 9th get a blank chip (Alt+digit only fires for 1–9),
/// keeping the column width aligned without showing an unreachable number.
fn number_chip(index: usize) -> Label {
    let text = if index < 9 {
        (index + 1).to_string()
    } else {
        String::new()
    };
    let num = Label::new(Some(&text));
    num.set_xalign(0.5);
    num.set_valign(gtk::Align::Center);
    num.add_css_class("cliccy-num");
    num
}

/// The leading chip for a row, or `None` when there's nothing to show on the
/// left. Images get a thumbnail; colour literals get a cheap Cairo-drawn swatch.
/// Other text shows no leading chip: the per-type glyph icons were dropped
/// because each meant an SVG rasterise per row (the cold-start cost), and the
/// content's kind is already clear from the text itself.
fn leading(entry: &Entry) -> Option<GtkBox> {
    let chip = GtkBox::new(Orientation::Horizontal, 0);
    chip.set_valign(gtk::Align::Center);
    // Explicitly non-expanding so the chip keeps its fixed box in the row even
    // though its child expands to fill (which would otherwise propagate up).
    chip.set_hexpand(false);

    match entry.kind {
        Kind::Image => {
            chip.add_css_class("cliccy-thumb");
            if let Some(pic) = entry.image.as_deref().and_then(thumbnail) {
                center_in_chip(&pic);
                chip.append(&pic);
            }
        }
        Kind::Text => {
            // Only colour literals keep a leading chip (the swatch); plain text
            // has none.
            let rgba = as_color(entry.text.as_deref().unwrap_or_default())?;
            chip.add_css_class("cliccy-kind");
            chip.add_css_class("color-chip");
            let sw = swatch(rgba);
            center_in_chip(&sw);
            chip.append(&sw);
        }
    }
    Some(chip)
}

/// Make a chip's child fill the chip box and sit dead-centre. The child expands
/// to the chip's fixed size, then centres its own (smaller) drawn content.
fn center_in_chip(child: &impl IsA<gtk::Widget>) {
    child.set_hexpand(true);
    child.set_vexpand(true);
    child.set_halign(gtk::Align::Center);
    child.set_valign(gtk::Align::Center);
}

/// The middle column: the text preview (plus a small sub-label for images).
fn body(entry: &Entry) -> GtkBox {
    let vbox = GtkBox::new(Orientation::Vertical, 2);
    vbox.set_hexpand(true);
    vbox.set_valign(gtk::Align::Center);

    let (main, sub, mono) = match entry.kind {
        Kind::Image => ("Image".to_string(), Some("PNG"), false),
        Kind::Text => {
            let text = entry.text.as_deref().unwrap_or_default();
            let preview: String = text.replace('\n', " ").chars().take(PREVIEW_CHARS).collect();
            // Code-like content (shell, links, paths, colours) reads better in a
            // monospace face, matching the design's mixed typography.
            let mono = as_color(text).is_some() || glyph_for(text) != ICON_TEXT;
            (preview, None, mono)
        }
    };

    let label = Label::new(Some(&main));
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.add_css_class("cliccy-text");
    if mono {
        label.add_css_class("mono");
    }
    vbox.append(&label);

    if let Some(sub) = sub {
        let s = Label::new(Some(sub));
        s.set_xalign(0.0);
        s.add_css_class("cliccy-sub");
        vbox.append(&s);
    }
    vbox
}

/// The trailing pin + delete buttons, revealed on row hover/selection by CSS.
fn actions(state: &Shared, entry: &Entry) -> GtkBox {
    let bar = GtkBox::new(Orientation::Horizontal, 2);
    bar.add_css_class("cliccy-actions");
    bar.set_valign(gtk::Align::Center);

    let pin = Button::new();
    pin.set_can_focus(false);
    pin.add_css_class("cliccy-act");
    pin.add_css_class("flat");
    if entry.pinned {
        pin.add_css_class("on");
    }
    pin.set_tooltip_text(Some(if entry.pinned {
        "Unpin (Ctrl+P)"
    } else {
        "Pin (Ctrl+P)"
    }));
    if let Some(img) = pin_icon(entry.pinned) {
        pin.set_child(Some(&img));
    }
    let s = state.clone();
    let id = entry.id;
    pin.connect_clicked(move |_| {
        let _ = s.store.toggle_pin(id);
        crate::ui::refresh(&s);
    });

    let del = Button::new();
    del.set_can_focus(false);
    del.add_css_class("cliccy-act");
    del.add_css_class("danger");
    del.add_css_class("flat");
    del.set_tooltip_text(Some("Remove (Del)"));
    if let Some(img) = stroke_icon(ICON_TRASH, OVERLAY, 15) {
        del.set_child(Some(&img));
    }
    let s = state.clone();
    let id = entry.id;
    del.connect_clicked(move |_| {
        let _ = s.store.delete(id);
        crate::ui::refresh(&s);
    });

    bar.append(&pin);
    bar.append(&del);
    bar
}

/// A "Pinned" / "Recent" section header, attached above the first row of each
/// group via `ListBox::set_header_func`.
pub fn group_header(pinned: bool) -> GtkBox {
    let hbox = GtkBox::new(Orientation::Horizontal, 7);
    hbox.add_css_class("cliccy-group");

    if pinned {
        if let Some(img) = pin_glyph(MAUVE, 11) {
            hbox.append(&img);
        }
    }
    let label = Label::new(Some(if pinned { "Pinned" } else { "Recent" }));
    label.add_css_class("cliccy-group-label");
    hbox.append(&label);

    // A hairline that fills the row, echoing the design's trailing rule.
    let line = gtk::Separator::new(Orientation::Horizontal);
    line.add_css_class("cliccy-group-line");
    line.set_hexpand(true);
    line.set_valign(gtk::Align::Center);
    hbox.append(&line);
    hbox
}

// ---- glyphs -------------------------------------------------------------

/// The header magnifier glyph (a circle + handle, so it needs its own builder
/// rather than the single-path `stroke_icon`).
pub fn search_glyph() -> Option<gtk::Image> {
    render_svg(
        &format!(
            "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' width='24' height='24'>\
             <g fill='none' stroke='{OVERLAY}' stroke-width='1.9' stroke-linecap='round' \
             stroke-linejoin='round'><circle cx='11' cy='11' r='7'/><path d='m21 21-4.3-4.3'/></g></svg>"
        ),
        17,
    )
}

const ICON_TEXT: &str = "M5 7h14M5 12h14M5 17h9";
const ICON_LINK: &str = "M9.5 13.5 14 9M8.5 11 6 13.5a3 3 0 0 0 4.2 4.3L12 16M15.5 13l1.5-1.5A3 3 0 0 0 12.8 7L11 8.8";
const ICON_TERMINAL: &str = "M5 7l4 4-4 4M12 16h6";
const ICON_TRASH: &str =
    "M4 7h16M9 7V5a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2M6 7l1 13a1 1 0 0 0 1 1h8a1 1 0 0 0 1-1l1-13";
const PIN_PATH: &str = "M12 17v5M9 4h6l-1 6 3 3H7l3-3-1-6Z";

/// Pick a type glyph for a text entry from a few cheap content heuristics.
fn glyph_for(text: &str) -> &'static str {
    let t = text.trim_start();
    if t.starts_with("http://") || t.starts_with("https://") || t.starts_with("www.") {
        ICON_LINK
    } else if looks_like_shell(t) {
        ICON_TERMINAL
    } else {
        ICON_TEXT
    }
}

fn looks_like_shell(t: &str) -> bool {
    const PREFIXES: [&str; 16] = [
        "sudo ", "git ", "cargo ", "npm ", "apt ", "cd ", "ls ", "./", "curl ", "docker ", "ssh ",
        "cp ", "mv ", "rm ", "mkdir ", "echo ",
    ];
    t.starts_with('$') || PREFIXES.iter().any(|p| t.starts_with(p))
}

/// Parse a leading `#hex` / `rgb()` colour literal into an `RGBA`, so it can be
/// shown as a swatch rather than a generic text glyph.
fn as_color(text: &str) -> Option<gdk::RGBA> {
    let t = text.trim();
    if t.len() > 30 || !(t.starts_with('#') || t.starts_with("rgb")) {
        return None;
    }
    gdk::RGBA::parse(t).ok()
}

fn pin_icon(filled: bool) -> Option<gtk::Image> {
    if filled {
        filled_icon(PIN_PATH, MAUVE, 15)
    } else {
        stroke_icon(PIN_PATH, OVERLAY, 15)
    }
}

fn pin_glyph(color: &str, px: i32) -> Option<gtk::Image> {
    filled_icon(PIN_PATH, color, px)
}

fn stroke_icon(path: &str, color: &str, px: i32) -> Option<gtk::Image> {
    render_svg(&svg(path, "none", color), px)
}

fn filled_icon(path: &str, color: &str, px: i32) -> Option<gtk::Image> {
    render_svg(&svg(path, color, color), px)
}

fn svg(path: &str, fill: &str, stroke: &str) -> String {
    format!(
        "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' width='24' height='24'>\
         <path d='{path}' fill='{fill}' stroke='{stroke}' stroke-width='1.9' \
         stroke-linecap='round' stroke-linejoin='round'/></svg>"
    )
}

/// Supersample factor for rasterised SVGs. Rendering the vector several times
/// larger than the on-screen size and letting GTK downsample yields crisp edges
/// (and stays sharp on HiDPI) instead of the soft look of rendering tiny.
const ICON_OVERSAMPLE: i32 = 4;

fn render_svg(markup: &str, px: i32) -> Option<gtk::Image> {
    // Same markup + size always rasterises to the same texture, so cache it: the
    // handful of distinct glyphs recur on every row of every refresh.
    let texture = cached_texture(format!("svg:{px}:{markup}"), || {
        let bytes = glib::Bytes::from(markup.as_bytes());
        let stream = gtk::gio::MemoryInputStream::from_bytes(&bytes);
        let pixbuf = gtk::gdk_pixbuf::Pixbuf::from_stream_at_scale(
            &stream,
            px * ICON_OVERSAMPLE,
            px * ICON_OVERSAMPLE,
            true,
            gtk::gio::Cancellable::NONE,
        )
        .ok()?;
        Some(gdk::Texture::for_pixbuf(&pixbuf))
    })?;
    let image = gtk::Image::from_paintable(Some(&texture));
    image.set_pixel_size(px);
    // Centre at the intended size so the chip box never stretches it.
    image.set_halign(gtk::Align::Center);
    image.set_valign(gtk::Align::Center);
    Some(image)
}

/// A rounded colour swatch drawn with Cairo (CSS can't take a per-row colour).
fn swatch(rgba: gdk::RGBA) -> DrawingArea {
    let area = DrawingArea::new();
    area.set_content_width(18);
    area.set_content_height(18);
    area.set_valign(gtk::Align::Center);
    area.set_halign(gtk::Align::Center);
    area.set_draw_func(move |_, cr, w, h| {
        rounded_rect(cr, w as f64, h as f64, 5.0);
        cr.set_source_rgba(
            rgba.red() as f64,
            rgba.green() as f64,
            rgba.blue() as f64,
            rgba.alpha() as f64,
        );
        let _ = cr.fill_preserve();
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.12);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
    });
    area
}

fn rounded_rect(cr: &cairo::Context, w: f64, h: f64, r: f64) {
    use std::f64::consts::PI;
    cr.new_sub_path();
    cr.arc(w - r, r, r, -PI / 2.0, 0.0);
    cr.arc(w - r, h - r, r, 0.0, PI / 2.0);
    cr.arc(r, h - r, r, PI / 2.0, PI);
    cr.arc(r, r, r, PI, 1.5 * PI);
    cr.close_path();
}

// ---- image thumbnail ----------------------------------------------------

/// Compact bounds for a row thumbnail — small, like the design's inline preview.
const THUMB_W: i32 = 54;
const THUMB_H: i32 = 34;
/// Decode at 2× the display size and show downsampled, so previews stay sharp.
const THUMB_SCALE: i32 = 2;

fn thumbnail(bytes: &[u8]) -> Option<gtk::Picture> {
    // Key by the image's content hash so the same picture isn't re-decoded on
    // every refresh while the popup is open or filtered.
    let texture = cached_texture(format!("thumb:{}", image_key(bytes)), || {
        let stream = gtk::gio::MemoryInputStream::from_bytes(&glib::Bytes::from(bytes));
        let pixbuf = gtk::gdk_pixbuf::Pixbuf::from_stream_at_scale(
            &stream,
            THUMB_W * THUMB_SCALE,
            THUMB_H * THUMB_SCALE,
            true,
            gtk::gio::Cancellable::NONE,
        )
        .ok()?;
        Some(gdk::Texture::for_pixbuf(&pixbuf))
    })?;
    let picture = gtk::Picture::for_paintable(&texture);
    picture.set_size_request(texture.width() / THUMB_SCALE, texture.height() / THUMB_SCALE);
    picture.set_halign(gtk::Align::Center);
    picture.set_valign(gtk::Align::Center);
    Some(picture)
}

/// Decode `bytes` (PNG) to a texture fitting a `box_px`×`box_px` square with
/// aspect preserved, cached by content hash. Backs the side-panel image preview
/// (`ui_preview`); the shared cache means repeated hovers over the same image
/// row reuse one decode instead of re-rasterising the PNG each time.
pub fn preview_texture(bytes: &[u8], box_px: i32) -> Option<gdk::Texture> {
    cached_texture(format!("preview:{box_px}:{}", image_key(bytes)), || {
        let stream = gtk::gio::MemoryInputStream::from_bytes(&glib::Bytes::from(bytes));
        let pixbuf = gtk::gdk_pixbuf::Pixbuf::from_stream_at_scale(
            &stream,
            box_px,
            box_px,
            true,
            gtk::gio::Cancellable::NONE,
        )
        .ok()?;
        Some(gdk::Texture::for_pixbuf(&pixbuf))
    })
}

// ---- relative time ------------------------------------------------------

/// Format a copy timestamp as a compact "time ago" (now / 9s / 3m / 2h / 5d).
fn time_ago(copied_at_ms: i64) -> String {
    let secs = (now_millis() - copied_at_ms).max(0) / 1000;
    if secs < 8 {
        "now".to_string()
    } else if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

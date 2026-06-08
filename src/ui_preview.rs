//! Full-content preview for a clipped row, shown the same way for mouse and
//! keyboard.
//!
//! The row list shows a width-clipped, newline-flattened preview, so long or
//! multi-line entries read as "…" and image rows shrink to a tiny thumbnail.
//! Hovering a row (pointer) or pressing Space on the selected row pops up a small
//! side panel: text entries show their complete text (scrollable, original line
//! breaks intact); image entries show the full PNG scaled to fit plus its pixel
//! dimensions and size. Both paths share one popover slot in `AppState`, so they
//! never stack and dismiss the same way.

use gtk::prelude::*;
use gtk::{
    pango, Box as GtkBox, EventControllerMotion, Label, ListBoxRow, Orientation, Popover,
    PositionType, ScrolledWindow,
};

use crate::app::Shared;
use crate::store::{Entry, Kind};
use crate::ui_row;

/// Below this length a single-line text entry almost always fits the row width
/// uncut, so a preview would only duplicate what's already visible. Above it (or
/// when the entry has newlines the row flattens to spaces) the row is likely
/// ellipsized and the preview earns its keep.
const MIN_PREVIEW_CHARS: usize = 50;

/// Upper bound on previewed text. Stored text is uncapped (a multi-MB paste is
/// kept whole); laying out that much in a single Pango label would jank, and the
/// panel only needs to show enough to read. Cap generously and mark truncation.
const MAX_PREVIEW_CHARS: usize = 20_000;

/// Long edge (px) the previewed image is scaled to fit — a fixed cap so the
/// floating preview stays bounded rather than showing the image at full size.
/// Smaller images are shown at native size. The texture is decoded at exactly
/// this size so a `Picture`'s natural size (its texture's intrinsic size) bounds
/// the popover; unlike the text path there is no scroller to cap the width.
const PREVIEW_IMG_MAX: i32 = 560;

/// What a row's side panel should display, or `None` when there's nothing worth
/// previewing (short single-line text the row already shows in full).
pub enum Preview {
    Text(String),
    /// The stored PNG bytes, rendered scaled-to-fit in the panel.
    Image(Vec<u8>),
}

/// Decide what (if anything) to preview for `entry`. Text routes through the
/// length/newline heuristic; images always preview (the row only shows a tiny
/// thumbnail, so the full image always earns the panel).
pub fn previewable(entry: &Entry) -> Option<Preview> {
    match entry.kind {
        Kind::Image => entry.image.clone().map(Preview::Image),
        Kind::Text => previewable_text(entry).map(Preview::Text),
    }
}

/// The text to preview for `entry`, or `None` when the row already shows
/// everything (short single-line text) or it carries no text (images). Newlines
/// are kept — unlike the flattened row preview, this reads as the original.
fn previewable_text(entry: &Entry) -> Option<String> {
    if entry.kind != Kind::Text {
        return None;
    }
    let text = entry.text.as_deref().unwrap_or_default().trim_end();
    if text.is_empty() {
        return None;
    }
    let count = text.chars().count();
    if count <= MIN_PREVIEW_CHARS && !text.contains('\n') {
        return None;
    }
    if count > MAX_PREVIEW_CHARS {
        let mut t: String = text.chars().take(MAX_PREVIEW_CHARS).collect();
        t.push_str("\n\n… (truncated)");
        Some(t)
    } else {
        Some(text.to_string())
    }
}

/// Open the preview for the selected row (the keyboard path), or close it if
/// already open. No-op when nothing is selected or the row isn't previewable.
pub fn toggle(state: &Shared) {
    if close(state) {
        return;
    }
    let Some(row) = state.list.selected_row() else {
        return;
    };
    let idx = row.index();
    if idx < 0 {
        return;
    }
    let preview = state.current.borrow().get(idx as usize).and_then(previewable);
    if let Some(preview) = preview {
        show_for_row(state, &row, &preview);
    }
}

/// Attach a pointer-hover preview to `row` (the mouse path), mirroring the
/// keyboard Space behaviour. Skipped for rows that already show everything, so
/// hovering short entries pops nothing.
pub fn attach_hover(state: &Shared, row: &ListBoxRow, entry: &Entry) {
    let Some(preview) = previewable(entry) else {
        return;
    };
    let motion = EventControllerMotion::new();

    // Weak row handle so the controller's closures don't pin the row alive after
    // a refresh removes it (the row owns the controller — a strong clone back to
    // the row would cycle).
    let row_weak = row.downgrade();
    let s = state.clone();
    motion.connect_enter(move |_, _, _| {
        if let Some(row) = row_weak.upgrade() {
            show_for_row(&s, &row, &preview);
        }
    });
    let s = state.clone();
    motion.connect_leave(move |_| {
        close(&s);
    });
    row.add_controller(motion);
}

/// Replace any open preview with one anchored to `row` showing `text`. If a
/// preview is already showing for this very row (a benign re-`enter`, e.g. a
/// list refresh re-firing under a stationary pointer), it's left as-is rather
/// than torn down and rebuilt.
fn show_for_row(state: &Shared, row: &ListBoxRow, preview: &Preview) {
    let same_row = state
        .preview
        .borrow()
        .as_ref()
        .and_then(|pop| pop.parent())
        .is_some_and(|parent| &parent == row.upcast_ref::<gtk::Widget>());
    if same_row {
        return;
    }
    close(state);
    let pop = build(row, preview);
    pop.popup();
    *state.preview.borrow_mut() = Some(pop);
}

/// Close the open preview, if any. Returns whether one was actually closed, so
/// callers (Esc handling) can tell "closed the preview" from "nothing to do".
/// Also called on navigation/refresh/hide so a popover never lingers pointing at
/// a row that's about to move or be removed.
pub fn close(state: &Shared) -> bool {
    if let Some(pop) = state.preview.borrow_mut().take() {
        pop.popdown();
        // Parented to a list row; unparent so removing that row on the next
        // refresh doesn't leave a dangling child.
        pop.unparent();
        true
    } else {
        false
    }
}

/// Build the preview popover anchored to `row`, showing `preview`'s content in a
/// bounded panel. Non-autohide so it doesn't grab focus (which would trip the
/// window's focus-out auto-hide) — its lifetime is driven entirely by the
/// hover/keyboard handlers.
fn build(row: &ListBoxRow, preview: &Preview) -> Popover {
    let pop = Popover::new();
    pop.set_parent(row);
    pop.set_autohide(false);
    pop.set_position(PositionType::Right);

    let child = match preview {
        Preview::Text(text) => {
            // Text keeps the framed, arrowed panel.
            pop.set_has_arrow(true);
            pop.add_css_class("cliccy-preview");
            build_text(text)
        }
        Preview::Image(bytes) => {
            // Images show bare: no panel background, border, or arrow — just the
            // image floating beside the row.
            pop.set_has_arrow(false);
            pop.add_css_class("cliccy-preview-img");
            build_image(bytes)
        }
    };
    pop.set_child(Some(&child));
    pop
}

/// The panel body for a text entry: the full text in a bounded, scrollable,
/// word-wrapping label.
fn build_text(text: &str) -> gtk::Widget {
    let label = Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_yalign(0.0);
    label.set_wrap(true);
    label.set_wrap_mode(pango::WrapMode::WordChar);
    label.set_max_width_chars(64);
    label.set_selectable(false);
    label.add_css_class("cliccy-preview-text");

    let scroller = ScrolledWindow::new();
    scroller.set_child(Some(&label));
    scroller.set_propagate_natural_width(true);
    scroller.set_propagate_natural_height(true);
    scroller.set_min_content_width(240);
    scroller.set_max_content_width(520);
    scroller.set_max_content_height(320);
    // Horizontal wraps via the label; only the vertical axis ever scrolls.
    scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroller.upcast()
}

/// The body for an image entry: just the PNG, scaled to fit `PREVIEW_IMG_MAX`
/// (native size when smaller) and shown bare (no panel chrome around it).
fn build_image(bytes: &[u8]) -> gtk::Widget {
    // Native dimensions drive the target so small images render at 1:1 instead of
    // being blown up to the cap (which would look soft).
    let long = png_dimensions(bytes)
        .map(|(w, h)| w.max(h).max(1) as i32)
        .unwrap_or(PREVIEW_IMG_MAX);
    let target = long.min(PREVIEW_IMG_MAX);

    if let Some(tex) = ui_row::preview_texture(bytes, target) {
        let pic = gtk::Picture::for_paintable(&tex);
        pic.set_halign(gtk::Align::Center);
        pic.set_valign(gtk::Align::Center);
        return pic.upcast();
    }
    // Decode failed (non-PNG or corrupt) — show nothing rather than a stray box.
    GtkBox::new(Orientation::Vertical, 0).upcast()
}

/// Pixel width/height from a PNG's IHDR header. Clipboard images are always PNG
/// (see `store::record_image`), so a cheap header read gives native dimensions
/// without decoding the whole image. A 0-dimension IHDR (corrupt/crafted PNG) is
/// no usable size and reads as unknown.
fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    if w == 0 || h == 0 {
        return None;
    }
    Some((w, h))
}

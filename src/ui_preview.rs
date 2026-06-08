//! Full-text preview for a clipped row, shown the same way for mouse and
//! keyboard.
//!
//! The row list shows a width-clipped, newline-flattened preview, so long or
//! multi-line entries read as "…". Hovering a row (pointer) or pressing Space on
//! the selected row pops up a small side panel with the entry's complete text
//! (scrollable, original line breaks intact). Both paths share one popover slot
//! in `AppState`, so they never stack and dismiss the same way.

use gtk::prelude::*;
use gtk::{
    pango, EventControllerMotion, Label, ListBoxRow, Popover, PositionType, ScrolledWindow,
};

use crate::app::Shared;
use crate::store::{Entry, Kind};

/// Below this length a single-line text entry almost always fits the row width
/// uncut, so a preview would only duplicate what's already visible. Above it (or
/// when the entry has newlines the row flattens to spaces) the row is likely
/// ellipsized and the preview earns its keep.
const MIN_PREVIEW_CHARS: usize = 50;

/// Upper bound on previewed text. Stored text is uncapped (a multi-MB paste is
/// kept whole); laying out that much in a single Pango label would jank, and the
/// panel only needs to show enough to read. Cap generously and mark truncation.
const MAX_PREVIEW_CHARS: usize = 20_000;

/// The text to preview for `entry`, or `None` when the row already shows
/// everything (short single-line text) or it carries no text (images). Newlines
/// are kept — unlike the flattened row preview, this reads as the original.
pub fn previewable_text(entry: &Entry) -> Option<String> {
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
    let text = state
        .current
        .borrow()
        .get(idx as usize)
        .and_then(previewable_text);
    if let Some(text) = text {
        show_for_row(state, &row, &text);
    }
}

/// Attach a pointer-hover preview to `row` (the mouse path), mirroring the
/// keyboard Space behaviour. Skipped for rows that already show everything, so
/// hovering short entries pops nothing.
pub fn attach_hover(state: &Shared, row: &ListBoxRow, entry: &Entry) {
    let Some(text) = previewable_text(entry) else {
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
            show_for_row(&s, &row, &text);
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
fn show_for_row(state: &Shared, row: &ListBoxRow, text: &str) {
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
    let pop = build(row, text);
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

/// Build the preview popover anchored to `row`, showing `text` whole in a
/// bounded, scrollable panel. Non-autohide so it doesn't grab focus (which would
/// trip the window's focus-out auto-hide) — its lifetime is driven entirely by
/// the hover/keyboard handlers.
fn build(row: &ListBoxRow, text: &str) -> Popover {
    let pop = Popover::new();
    pop.set_parent(row);
    pop.set_autohide(false);
    pop.set_position(PositionType::Right);
    pop.set_has_arrow(true);
    pop.add_css_class("cliccy-preview");

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

    pop.set_child(Some(&scroller));
    pop
}

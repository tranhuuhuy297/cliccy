//! Keyboard navigation for the popup window.
//!
//! Handled in the capture phase so navigation keys work even while the search
//! entry holds focus; printable characters fall through to the entry for typing.
//!
//! Bindings: Up/Down move selection, Enter copies, Esc hides, Delete removes the
//! selected entry, Ctrl+P toggles its pin, and Alt+1..9 quick-picks a row.

use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::app::Shared;
use crate::ui;

pub fn handle(state: &Shared, keyval: gdk::Key, modifier: gdk::ModifierType) -> glib::Propagation {
    use gdk::Key;
    match keyval {
        Key::Escape => stop(ui::hide(state)),
        Key::Down => stop(move_selection(state, 1)),
        Key::Up => stop(move_selection(state, -1)),
        Key::Return | Key::KP_Enter => stop(activate_selected(state)),
        Key::Delete => stop(delete_selected(state)),
        Key::p | Key::P if modifier.contains(gdk::ModifierType::CONTROL_MASK) => {
            stop(toggle_pin_selected(state))
        }
        _ => {
            if modifier.contains(gdk::ModifierType::ALT_MASK) {
                if let Some(d) = keyval.to_unicode().and_then(|c| c.to_digit(10)) {
                    if (1..=9).contains(&d) {
                        select_index(state, (d - 1) as usize);
                        activate_selected(state);
                        return glib::Propagation::Stop;
                    }
                }
            }
            glib::Propagation::Proceed
        }
    }
}

fn stop(_: ()) -> glib::Propagation {
    glib::Propagation::Stop
}

fn move_selection(state: &Shared, delta: i32) {
    let count = state.current.borrow().len() as i32;
    if count == 0 {
        return;
    }
    let current = state.list.selected_row().map(|r| r.index()).unwrap_or(-1);
    let next = (current + delta).clamp(0, count - 1);
    if let Some(row) = state.list.row_at_index(next) {
        state.list.select_row(Some(&row));
    }
}

fn select_index(state: &Shared, index: usize) {
    if let Some(row) = state.list.row_at_index(index as i32) {
        state.list.select_row(Some(&row));
    }
}

fn selected_entry_id(state: &Shared) -> Option<i64> {
    let index = state.list.selected_row()?.index() as usize;
    state.current.borrow().get(index).map(|e| e.id)
}

fn activate_selected(state: &Shared) {
    let index = state.list.selected_row().map(|r| r.index() as usize);
    let entry = index.and_then(|i| state.current.borrow().get(i).cloned());
    if let Some(entry) = entry {
        ui::copy_entry(state, &entry);
    }
}

fn delete_selected(state: &Shared) {
    if let Some(id) = selected_entry_id(state) {
        let _ = state.store.delete(id);
        ui::refresh(state);
    }
}

fn toggle_pin_selected(state: &Shared) {
    if let Some(id) = selected_entry_id(state) {
        let _ = state.store.toggle_pin(id);
        ui::refresh(state);
    }
}

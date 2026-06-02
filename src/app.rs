//! GTK application bootstrap, shared state, and single-instance command routing.
//!
//! The first launch (`cliccy daemon`) becomes the GApplication primary instance:
//! it builds the popup window, starts clipboard monitoring, and `hold()`s itself
//! alive with no visible window. Subsequent invocations such as `cliccy toggle`
//! are forwarded by GApplication to this primary instance's command-line handler.

use gtk::prelude::*;
use gtk::{gio, glib, Application};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::clipboard_backend::Backend;
use crate::store::{Entry, Store};
use crate::{config, monitor, ui};

/// Shared, single-threaded application state passed into every GTK closure.
pub struct AppState {
    pub store: Store,
    pub window: gtk::ApplicationWindow,
    /// Bare text widget (not a themed `SearchEntry`) so the search field can be
    /// styled flat to match the design instead of carrying Adwaita entry chrome.
    pub search: gtk::Text,
    pub list: gtk::ListBox,
    /// Clipboard read/write backend chosen for this session (Wayland or X11).
    pub backend: Backend,
    /// Entries currently shown in the list, in row order (for index lookups).
    pub current: RefCell<Vec<Entry>>,
    /// Last clipboard value the poller observed, used to detect real changes
    /// and to suppress re-recording our own copy-backs.
    pub last_seen: RefCell<Option<String>>,
    /// Keeps the GApplication alive while the window is hidden. Dropping this
    /// guard releases the hold and lets the daemon exit, so it lives as long
    /// as the shared state does.
    pub hold: RefCell<Option<gio::ApplicationHoldGuard>>,
    /// True during the show transition (before the window first gains focus),
    /// so the focus-out auto-hide doesn't fire on the brief unfocused moment
    /// while the popup is still being raised.
    pub suppress_focus_hide: Cell<bool>,
}

pub type Shared = Rc<AppState>;

/// Build and run the GTK application. Returns the process exit code.
pub fn run() -> glib::ExitCode {
    let app = Application::builder()
        .application_id(config::APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    let state: Rc<RefCell<Option<Shared>>> = Rc::new(RefCell::new(None));

    // Runs once, only in the primary instance.
    let startup_state = state.clone();
    app.connect_startup(move |app| {
        let shared = ui::build(app);
        monitor::install(&shared);
        // Keep the daemon resident even though the window starts hidden; the
        // guard is stored so it is not dropped at the end of this closure.
        *shared.hold.borrow_mut() = Some(app.hold());
        *startup_state.borrow_mut() = Some(shared);
    });

    // Runs for every invocation; remote ones are forwarded here by GApplication.
    let cmd_state = state.clone();
    app.connect_command_line(move |_app, cmdline| {
        let args = cmdline.arguments();
        let verb = args
            .get(1)
            .and_then(|a| a.to_str())
            .unwrap_or("daemon")
            .to_string();
        if let Some(shared) = cmd_state.borrow().as_ref() {
            match verb.as_str() {
                "toggle" => ui::toggle(shared),
                "show" => ui::show(shared),
                "hide" => ui::hide(shared),
                // "daemon" (or anything else): stay resident, do nothing visible.
                _ => {}
            }
        }
        0
    });

    app.run()
}

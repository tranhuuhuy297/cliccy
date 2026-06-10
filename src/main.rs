//! Cliccy — a Maccy-style clipboard history manager for Linux (GTK4).
//!
//! Subcommands:
//!   cliccy [daemon]          run the resident clipboard monitor + popup (default)
//!   cliccy toggle            show/hide the popup (bind a global hotkey to this)
//!   cliccy show | hide       force the popup open/closed
//!   cliccy clear             delete all unpinned history
//!   cliccy install-hotkey    register a GNOME shortcut (default <Control><Alt>V)
//!   cliccy uninstall-hotkey  remove the GNOME shortcut
//!   cliccy version | help

mod app;
mod clipboard_backend;
mod config;
mod hotkey;
mod icon_cache;
mod keys;
mod monitor;
mod store;
mod tray;
mod ui;
mod ui_preview;
mod ui_row;
mod x11_clipboard_watch;
mod x11_window_hints;

use gtk::glib;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let verb = args.get(1).map(String::as_str).unwrap_or("daemon");

    match verb {
        "help" | "-h" | "--help" => {
            print_help();
            ExitCode::SUCCESS
        }
        "version" | "-v" | "--version" => {
            println!("cliccy {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        "clear" => clear_history(),
        "install-hotkey" => hotkey::install_hotkey(args.get(2).map(String::as_str)),
        "uninstall-hotkey" => hotkey::uninstall_hotkey(),
        // GTK-driven verbs: the first becomes the daemon, the rest forward to it.
        "daemon" | "toggle" | "show" | "hide" => {
            // GTK derives the window's identity from the program name: the X11
            // WM_CLASS and the Wayland app_id both come from prgname, which would
            // otherwise default to argv[0]'s basename ("cliccy"). GNOME maps a
            // window to its dock/Alt-Tab icon by matching that identity to a
            // desktop file of the same name; our entry is com.cliccy.Cliccy
            // (.desktop + StartupWMClass + installed icon all use the app id), so
            // a bare "cliccy" identity misses it and the dock shows the generic
            // fallback icon. Pin prgname to the app id here, before GTK opens the
            // display, so the match succeeds. Set before Application::run, which
            // only auto-assigns prgname when it is still unset.
            glib::set_prgname(Some(config::APP_ID));
            // Render under XWayland so the popup can be marked skip-taskbar and
            // stay out of the GNOME dock. Falls back to Wayland if X is absent.
            if std::env::var_os("GDK_BACKEND").is_none() {
                std::env::set_var("GDK_BACKEND", "x11,wayland");
            }
            run_gtk()
        }
        other => {
            eprintln!("cliccy: unknown command '{other}'\n");
            print_help();
            ExitCode::FAILURE
        }
    }
}

fn run_gtk() -> ExitCode {
    match app::run() {
        code if code == glib::ExitCode::SUCCESS => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    }
}

fn clear_history() -> ExitCode {
    match store::Store::open(&config::db_path()).and_then(|s| s.clear()) {
        Ok(()) => {
            println!("Cleared clipboard history (pinned items kept).");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("cliccy: failed to clear history: {e}");
            ExitCode::FAILURE
        }
    }
}

fn print_help() {
    println!(
        "Cliccy — clipboard history manager\n\n\
         USAGE:\n  \
         cliccy [COMMAND]\n\n\
         COMMANDS:\n  \
         daemon             Run the resident monitor + popup (default)\n  \
         toggle             Show/hide the popup (bind a global hotkey to this)\n  \
         show | hide        Force the popup open or closed\n  \
         clear              Delete all unpinned history\n  \
         install-hotkey     Register a GNOME shortcut (default <Control><Alt>V)\n  \
         uninstall-hotkey   Remove the GNOME shortcut\n  \
         version | help     Show version or this help"
    );
}

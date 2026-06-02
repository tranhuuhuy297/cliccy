//! Global hotkey installation via GNOME custom keybindings.
//!
//! Wayland forbids apps from grabbing global shortcuts directly, so instead we
//! register a GNOME custom keybinding (works on both X11 and Wayland sessions)
//! that runs `cliccy toggle`. Non-GNOME desktops must bind the command manually.

use std::process::{Command, ExitCode};

const SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys";
const KB_PATH: &str = "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/cliccy/";
// Lowercase 'v': GNOME's accelerator parser treats an uppercase letter as
// also requiring Shift, so "<Control><Alt>V" would only fire on Ctrl+Alt+Shift+V.
const DEFAULT_BINDING: &str = "<Control><Alt>v";

/// Register (or update) the GNOME custom keybinding for Cliccy.
pub fn install_hotkey(binding: Option<&str>) -> ExitCode {
    let binding = binding.unwrap_or(DEFAULT_BINDING);
    let command = format!("{} toggle", current_binary());

    if let Err(e) = ensure_path_registered() {
        eprintln!("error: {e}");
        return ExitCode::FAILURE;
    }

    let schema = format!("{SCHEMA}.custom-keybinding:{KB_PATH}");
    let ok = set(&schema, "name", "Cliccy")
        && set(&schema, "command", &command)
        && set(&schema, "binding", binding);

    if ok {
        println!("Hotkey installed: {binding} → {command}");
        ExitCode::SUCCESS
    } else {
        eprintln!("error: failed to set keybinding (is `gsettings` available / are you on GNOME?)");
        ExitCode::FAILURE
    }
}

/// Remove the Cliccy custom keybinding.
pub fn uninstall_hotkey() -> ExitCode {
    let mut paths = registered_paths();
    paths.retain(|p| p != KB_PATH);
    let _ = set(SCHEMA, "custom-keybindings", &format_list(&paths));
    println!("Hotkey removed.");
    ExitCode::SUCCESS
}

fn ensure_path_registered() -> Result<(), String> {
    let mut paths = registered_paths();
    if paths.iter().any(|p| p == KB_PATH) {
        return Ok(());
    }
    paths.push(KB_PATH.to_string());
    if set(SCHEMA, "custom-keybindings", &format_list(&paths)) {
        Ok(())
    } else {
        Err("could not update the custom-keybindings list".into())
    }
}

fn registered_paths() -> Vec<String> {
    let output = Command::new("gsettings")
        .args(["get", SCHEMA, "custom-keybindings"])
        .output();
    let raw = match output {
        Ok(o) => String::from_utf8_lossy(&o.stdout).into_owned(),
        Err(_) => return Vec::new(),
    };
    // Formats observed: "@as []" or "['/path/a/', '/path/b/']".
    raw.trim()
        .trim_start_matches("@as")
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(|s| s.trim().trim_matches('\'').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn format_list(paths: &[String]) -> String {
    if paths.is_empty() {
        return "[]".to_string();
    }
    let inner: Vec<String> = paths.iter().map(|p| format!("'{p}'")).collect();
    format!("[{}]", inner.join(", "))
}

fn set(schema: &str, key: &str, value: &str) -> bool {
    Command::new("gsettings")
        .args(["set", schema, key, value])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn current_binary() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(str::to_string))
        .unwrap_or_else(|| "cliccy".to_string())
}

# Cliccy

A lightweight clipboard history manager for Linux — a Maccy-style popup, built
with **Rust + GTK4**. Works on both X11 and Wayland (tested target: Ubuntu 22.04+).

![type-to-search popup with keyboard-driven history](https://example.invalid/placeholder)

## Features

- Resident daemon that records everything you copy (text)
- Fast type-to-search popup
- Full keyboard control: arrows, Enter, Esc, Alt+1–9 quick pick
- Pin frequently used snippets (`Ctrl+P`); pinned items never expire
- Delete a single entry (`Delete`) or clear all unpinned (`cliccy clear`)
- SQLite-backed, capped at 200 unpinned entries
- Single small binary, dark Catppuccin theme

## Requirements

- Rust (stable) and Cargo
- GTK4 development headers:
  - Debian/Ubuntu: `sudo apt install libgtk-4-dev`
  - Fedora: `sudo dnf install gtk4-devel`
- Clipboard CLI tools (used at runtime to read/write the clipboard):
  - `xclip` — primary, used whenever `DISPLAY` is set (incl. XWayland on GNOME)
  - `wl-clipboard` (`wl-copy`/`wl-paste`) — fallback for pure-Wayland sessions
  - Debian/Ubuntu: `sudo apt install xclip wl-clipboard`
- SQLite is bundled — no extra package needed.

## Install

One line — build + install + hotkey + autostart (clones the repo for you):

```bash
curl -fsSL https://raw.githubusercontent.com/tranhuuhuy297/cliccy/main/install.sh | bash
```

Pick your own hotkey by appending `-s -- '<Super>V'`. Or, from a checkout:

```bash
./install.sh                 # build + install + hotkey + autostart
./install.sh '<Super>V'      # optional: choose your own hotkey
```

This installs the binary to `~/.local/bin/cliccy`, registers a GNOME global
shortcut (default **Ctrl+Alt+V**), adds a login autostart entry, and launches
the daemon. Ensure `~/.local/bin` is on your `PATH`.

## Manual build

```bash
cargo build --release
./target/release/cliccy daemon &     # start the background monitor
./target/release/cliccy toggle       # open/close the popup
```

## Usage

| Command                    | Description                                        |
|----------------------------|----------------------------------------------------|
| `cliccy` / `cliccy daemon` | Run the resident monitor + popup (default)         |
| `cliccy toggle`            | Show/hide the popup (bind a global hotkey to this)  |
| `cliccy show` / `hide`     | Force the popup open / closed                       |
| `cliccy clear`             | Delete all unpinned history                         |
| `cliccy install-hotkey`    | Register a GNOME shortcut (default `<Control><Alt>V`) |
| `cliccy uninstall-hotkey`  | Remove the GNOME shortcut                           |

### In the popup

| Key            | Action                          |
|----------------|---------------------------------|
| Type           | Filter history                  |
| ↑ / ↓          | Move selection                  |
| Enter / click  | Copy selected entry, close      |
| Alt+1 … Alt+9  | Quick-pick that row             |
| Ctrl+P         | Pin / unpin selected entry      |
| Delete         | Remove selected entry           |
| Esc            | Close the popup                 |

## How it works

- A single `cliccy` process is the GApplication primary instance (the daemon).
  It watches the clipboard **event-driven** via X11 XFIXES and stores changes in
  `~/.local/share/cliccy/history.db`.
- `cliccy toggle` is forwarded by GApplication to the running daemon, which
  shows or hides the popup. Bind it to a global shortcut.
- On Wayland, apps can't grab global hotkeys directly, so the hotkey is a GNOME
  custom keybinding that runs `cliccy toggle`. Non-GNOME desktops: bind the
  command yourself in your compositor/WM settings.

### Why XFIXES, not polling

GNOME's Mutter does not implement the wlroots data-control protocol, so GDK's
clipboard `changed` signal never fires for an unfocused window. Two naive
workarounds both misbehave:

- **Polling with `wl-paste`** makes GNOME briefly create a *focus-grabbing*
  surface on every read, yanking focus from your active window — a constant
  "jitter".
- **Polling with `xclip`** avoids the focus theft (it reads via Mutter's
  XWayland clipboard bridge), but spawning a reader process every tick still
  causes a small, repeated stutter.

### No dock icon

A normal Wayland window shows an icon in the GNOME dock while open, and the dock
reflows (a small "jerk") when it closes. Wayland has no way for an app to opt out
of the taskbar, so Cliccy renders the popup under XWayland (`GDK_BACKEND=x11`) and
sets the EWMH hints Mutter honours — `_NET_WM_WINDOW_TYPE_UTILITY` plus
`SKIP_TASKBAR`/`SKIP_PAGER`. GNOME then keeps it out of the dock entirely, like a
proper menu-bar utility. If X is unavailable it falls back to the Wayland backend
(dock icon returns, but it still works).

### Why XFIXES, not polling

Cliccy listens for X11 **XFIXES** "selection owner changed" events.
Under XWayland, Mutter mirrors the Wayland selection onto the X CLIPBOARD and
takes ownership when it changes, which raises that event — so Cliccy reads the
clipboard *only when it actually changes*. When idle it does nothing: no timer,
no process spawning, no jitter. The one-shot read on change goes through `xclip`
(focus-safe via the bridge); copy-backs are written with `xclip` too. If X is
unreachable (pure-Wayland, no XWayland), it falls back to `wl-clipboard`
polling.

## Non-GNOME desktops

`install-hotkey` only supports GNOME. On KDE, Sway, Hyprland, etc., bind your
compositor's shortcut system to run `cliccy toggle`.

## License

MIT

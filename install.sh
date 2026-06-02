#!/usr/bin/env bash
# Build Cliccy, install it to ~/.local/bin, register the GNOME hotkey, and
# set it to autostart at login. Re-run any time to update.
#
# One-line install (no checkout needed):
#   curl -fsSL https://raw.githubusercontent.com/tranhuuhuy297/cliccy/main/install.sh | bash
# Pass a custom hotkey by appending:  | bash -s -- '<Super>V'
set -euo pipefail

REPO_URL="https://github.com/tranhuuhuy297/cliccy.git"
BIN_DIR="${HOME}/.local/bin"
AUTOSTART_DIR="${HOME}/.config/autostart"
HOTKEY="${1:-<Control><Alt>v}"

# Resolve the source tree. When run from a local checkout we build that; when
# piped straight from curl (no local source) we clone the repo into a cache dir
# first, so the one-line install above works.
self_dir="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" 2>/dev/null && pwd || true)"
if [ -n "${self_dir}" ] && [ -f "${self_dir}/Cargo.toml" ]; then
    SCRIPT_DIR="${self_dir}"
else
    if ! command -v git >/dev/null; then
        echo "Missing git, needed for the one-line install. Install git, or clone"
        echo "the repo and run ./install.sh from inside it."
        exit 1
    fi
    SCRIPT_DIR="${XDG_CACHE_HOME:-${HOME}/.cache}/cliccy-src"
    if [ -d "${SCRIPT_DIR}/.git" ]; then
        echo "==> Updating Cliccy source in ${SCRIPT_DIR}"
        git -C "${SCRIPT_DIR}" pull --ff-only
    else
        echo "==> Cloning Cliccy into ${SCRIPT_DIR}"
        git clone --depth 1 "${REPO_URL}" "${SCRIPT_DIR}"
    fi
fi

echo "==> Checking build dependencies"
if ! pkg-config --exists gtk4; then
    echo "Missing GTK4 development headers."
    echo "Install them first, e.g.:"
    echo "  sudo apt install libgtk-4-dev   # Debian/Ubuntu"
    echo "  sudo dnf install gtk4-devel      # Fedora"
    exit 1
fi

echo "==> Checking runtime clipboard tools"
if ! command -v xclip >/dev/null && ! command -v wl-paste >/dev/null; then
    echo "Missing a clipboard tool. Install at least one (xclip recommended):"
    echo "  sudo apt install xclip wl-clipboard   # Debian/Ubuntu"
    exit 1
fi
if ! command -v xclip >/dev/null; then
    echo "   NOTE: xclip not found. On GNOME/Wayland, install it to avoid the"
    echo "   focus 'jitter' caused by wl-paste:  sudo apt install xclip"
fi

echo "==> Building release binary"
( cd "${SCRIPT_DIR}" && cargo build --release )

echo "==> Installing binary to ${BIN_DIR}/cliccy"
mkdir -p "${BIN_DIR}"
install -m 755 "${SCRIPT_DIR}/target/release/cliccy" "${BIN_DIR}/cliccy"

echo "==> Installing app icon"
ICON_DIR="${HOME}/.local/share/icons/hicolor/scalable/apps"
mkdir -p "${ICON_DIR}"
install -m 644 "${SCRIPT_DIR}/assets/com.cliccy.Cliccy.svg" "${ICON_DIR}/com.cliccy.Cliccy.svg"
# Refresh the icon cache so launchers pick the icon up immediately (best-effort).
gtk-update-icon-cache -qtf "${HOME}/.local/share/icons/hicolor" 2>/dev/null || true

echo "==> Writing autostart entry"
mkdir -p "${AUTOSTART_DIR}"
cat > "${AUTOSTART_DIR}/cliccy.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Cliccy
Comment=Clipboard history manager
Exec=${BIN_DIR}/cliccy daemon
Icon=com.cliccy.Cliccy
Terminal=false
X-GNOME-Autostart-enabled=true
EOF

echo "==> Registering global hotkey (${HOTKEY})"
"${BIN_DIR}/cliccy" install-hotkey "${HOTKEY}" || \
    echo "   (skipped — not on GNOME? bind '${BIN_DIR}/cliccy toggle' manually)"

echo "==> Starting the daemon now"
pkill -f "cliccy daemon" 2>/dev/null || true
nohup "${BIN_DIR}/cliccy" daemon >/dev/null 2>&1 &

echo
echo "Done. Press ${HOTKEY} to open Cliccy."
echo "Make sure ${BIN_DIR} is on your PATH."

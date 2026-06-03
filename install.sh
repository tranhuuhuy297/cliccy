#!/usr/bin/env bash
# Install Cliccy: download the prebuilt binary (or build from source), register
# the GNOME hotkey, and set it to autostart at login. Re-run any time to update.
#
# One-line install (no checkout needed):
#   curl -fsSL https://raw.githubusercontent.com/tranhuuhuy297/cliccy/main/install.sh | bash
# Pass a custom hotkey by appending:  | bash -s -- '<Super>V'
# Force a from-source build:          CLICCY_FROM_SOURCE=1 ... | bash
set -euo pipefail

REPO_SLUG="tranhuuhuy297/cliccy"
REPO_URL="https://github.com/${REPO_SLUG}.git"
BIN_DIR="${HOME}/.local/bin"
AUTOSTART_DIR="${HOME}/.config/autostart"
ICON_DIR="${HOME}/.local/share/icons/hicolor/scalable/apps"
HOTKEY="${1:-<Control><Alt>v}"
FROM_SOURCE="${CLICCY_FROM_SOURCE:-}"

# Release asset names produced by .github/workflows/release.yml.
ASSET_BINARY="cliccy-linux-x86_64"
ASSET_ICON="com.cliccy.Cliccy.svg"

# Populated by obtain_*: the binary to install and the icon to install (icon
# optional — may stay empty if a prebuilt download omits it).
BUILT_BIN=""
ICON_SRC=""
WORK=""
cleanup() { [ -n "${WORK}" ] && rm -rf "${WORK}"; }
trap cleanup EXIT

# --- Acquire the binary -------------------------------------------------------

# Download the prebuilt binary + icon from the latest GitHub Release. Returns
# non-zero (so the caller falls back to a source build) on any miss: unsupported
# arch, no release yet, network failure, or checksum mismatch.
obtain_prebuilt() {
    local arch; arch="$(uname -m)"
    if [ "${arch}" != "x86_64" ]; then
        echo "   No prebuilt binary for ${arch}; building from source instead."
        return 1
    fi
    if ! command -v curl >/dev/null; then
        return 1
    fi

    WORK="$(mktemp -d)"
    local base="https://github.com/${REPO_SLUG}/releases/latest/download"
    echo "==> Downloading prebuilt binary (${arch})"
    if ! curl -fsSL "${base}/${ASSET_BINARY}" -o "${WORK}/cliccy"; then
        echo "   No prebuilt release found; building from source instead."
        return 1
    fi

    # Verify the checksum when the release publishes one (best-effort).
    if curl -fsSL "${base}/${ASSET_BINARY}.sha256" -o "${WORK}/sha256" \
        && command -v sha256sum >/dev/null; then
        local want got
        want="$(tr -d '[:space:]' < "${WORK}/sha256")"
        got="$(sha256sum "${WORK}/cliccy" | awk '{print $1}')"
        if [ -n "${want}" ] && [ "${want}" != "${got}" ]; then
            echo "   Checksum mismatch on prebuilt binary; building from source."
            return 1
        fi
    fi

    curl -fsSL "${base}/${ASSET_ICON}" -o "${WORK}/icon.svg" 2>/dev/null || true
    chmod +x "${WORK}/cliccy"
    BUILT_BIN="${WORK}/cliccy"
    [ -s "${WORK}/icon.svg" ] && ICON_SRC="${WORK}/icon.svg"
    return 0
}

# Build from a local checkout if we're running inside one, otherwise clone the
# repo into a cache dir and build that.
build_from_source() {
    local self_dir src_dir
    self_dir="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" 2>/dev/null && pwd || true)"
    if [ -n "${self_dir}" ] && [ -f "${self_dir}/Cargo.toml" ]; then
        src_dir="${self_dir}"
    else
        if ! command -v git >/dev/null; then
            echo "Missing git, needed to fetch the source. Install git, or clone"
            echo "the repo and run ./install.sh from inside it."
            exit 1
        fi
        src_dir="${XDG_CACHE_HOME:-${HOME}/.cache}/cliccy-src"
        if [ -d "${src_dir}/.git" ]; then
            echo "==> Updating Cliccy source in ${src_dir}"
            git -C "${src_dir}" pull --ff-only
        else
            echo "==> Cloning Cliccy into ${src_dir}"
            git clone --depth 1 "${REPO_URL}" "${src_dir}"
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

    echo "==> Building release binary"
    ( cd "${src_dir}" && cargo build --release )
    BUILT_BIN="${src_dir}/target/release/cliccy"
    ICON_SRC="${src_dir}/assets/${ASSET_ICON}"
}

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

# A local checkout means the user wants their working tree built; honor that.
# Otherwise prefer the prebuilt binary, falling back to source on any miss.
self_dir="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" 2>/dev/null && pwd || true)"
if [ -n "${FROM_SOURCE}" ] || { [ -n "${self_dir}" ] && [ -f "${self_dir}/Cargo.toml" ]; }; then
    build_from_source
else
    obtain_prebuilt || build_from_source
fi

# --- Install ------------------------------------------------------------------

echo "==> Installing binary to ${BIN_DIR}/cliccy"
mkdir -p "${BIN_DIR}"
install -m 755 "${BUILT_BIN}" "${BIN_DIR}/cliccy"

if [ -n "${ICON_SRC}" ] && [ -f "${ICON_SRC}" ]; then
    echo "==> Installing app icon"
    mkdir -p "${ICON_DIR}"
    install -m 644 "${ICON_SRC}" "${ICON_DIR}/${ASSET_ICON}"
    # Refresh the icon cache so launchers pick the icon up immediately (best-effort).
    gtk-update-icon-cache -qtf "${HOME}/.local/share/icons/hicolor" 2>/dev/null || true
fi

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
pkill -f "${BIN_DIR}/cliccy daemon" 2>/dev/null || true
nohup "${BIN_DIR}/cliccy" daemon >/dev/null 2>&1 &

# Give the daemon a moment to register on the session bus and build its window,
# then open the popup once. This warms the first (slowest) window map so the
# first real hotkey press is responsive, and shows the user Cliccy is working.
sleep 2
"${BIN_DIR}/cliccy" show >/dev/null 2>&1 || true

echo
echo "Done. Cliccy is open — press ${HOTKEY} any time to toggle it."
echo "Make sure ${BIN_DIR} is on your PATH."

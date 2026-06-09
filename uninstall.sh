#!/usr/bin/env bash
# Remove Cliccy: stop the daemon, drop the GNOME hotkey, and delete the binary,
# autostart entry, and app icon. History in ~/.local/share/cliccy is kept unless
# you pass --purge.
#
# One-line uninstall (no checkout needed):
#   curl -fsSL https://raw.githubusercontent.com/tranhuuhuy297/cliccy/main/uninstall.sh | bash
# Also wipe clipboard history:  | bash -s -- --purge
set -euo pipefail

BIN_DIR="${HOME}/.local/bin"
AUTOSTART_DIR="${HOME}/.config/autostart"
APPLICATIONS_DIR="${HOME}/.local/share/applications"
ICON_DIR="${HOME}/.local/share/icons/hicolor/scalable/apps"
DATA_DIR="${HOME}/.local/share/cliccy"
KB_PATH="/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/cliccy/"
SCHEMA="org.gnome.settings-daemon.plugins.media-keys"

echo "==> Stopping the daemon"
# Match the exact process name so this catches the daemon wherever it runs from
# (installed binary or a target/debug build), not just the installed path.
pkill -x cliccy 2>/dev/null || true

echo "==> Removing GNOME hotkey"
if command -v gsettings >/dev/null; then
    current="$(gsettings get "${SCHEMA}" custom-keybindings 2>/dev/null || echo '@as []')"
    if printf '%s' "${current}" | grep -q "${KB_PATH}"; then
        # Drop our path from the list; reset to empty if it was the only one.
        filtered="$(printf '%s' "${current}" | sed "s#'${KB_PATH}',\? *##g")"
        gsettings set "${SCHEMA}" custom-keybindings "${filtered}" 2>/dev/null || \
            gsettings reset "${SCHEMA}" custom-keybindings 2>/dev/null || true
    fi
fi

echo "==> Removing files"
rm -fv "${BIN_DIR}/cliccy" \
       "${AUTOSTART_DIR}/cliccy.desktop" \
       "${APPLICATIONS_DIR}/com.cliccy.Cliccy.desktop" \
       "${ICON_DIR}/com.cliccy.Cliccy.svg" 2>/dev/null || true
gtk-update-icon-cache -qtf "${HOME}/.local/share/icons/hicolor" 2>/dev/null || true
update-desktop-database -q "${APPLICATIONS_DIR}" 2>/dev/null || true

if [ "${1:-}" = "--purge" ]; then
    echo "==> Purging clipboard history (${DATA_DIR})"
    rm -rf "${DATA_DIR}"
else
    echo "==> Keeping clipboard history (${DATA_DIR}); pass --purge to wipe it"
fi

echo
echo "Done. Cliccy has been uninstalled."

#!/usr/bin/env bash
# Installs ADM for the current user only (no root): the binary in ~/.local/bin, the icon, and a launcher
# entry so it shows up in the app menu / search. Run it again after rebuilding to update the binary.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
bin="$root/target/release/adm"
[ -x "$bin" ] || { echo "Build it first:  cargo build --release -p adm" >&2; exit 1; }

install -Dm755 "$bin" "$HOME/.local/bin/adm"
install -Dm644 "$root/src-tauri/icons/128x128.png" "$HOME/.local/share/icons/hicolor/128x128/apps/adm.png"
install -Dm644 "$root/src-tauri/icons/icon.png"    "$HOME/.local/share/icons/hicolor/512x512/apps/adm.png"

mkdir -p "$HOME/.local/share/applications"
# The icon is given as a full path (like XDM does): some bars and launchers only look in theme folders that
# have an index.theme, which the per-user hicolor folder does not.
cat > "$HOME/.local/share/applications/adm.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Version=1.0
Name=ADM
GenericName=Download Manager
Comment=Fast, lightweight download manager (Axenith Download Manager)
Keywords=ADM;Axenith;Download;Downloader;Manager;Accelerator;Resume;XDM;IDM;
Exec=$HOME/.local/bin/adm %u
Icon=$HOME/.local/share/icons/hicolor/512x512/apps/adm.png
Terminal=false
Categories=Network;FileTransfer;
StartupWMClass=adm
DESKTOP

command -v desktop-file-validate >/dev/null && desktop-file-validate "$HOME/.local/share/applications/adm.desktop"
command -v update-desktop-database >/dev/null && update-desktop-database "$HOME/.local/share/applications" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t -f "$HOME/.local/share/icons/hicolor" 2>/dev/null || true
echo "Installed. Search for \"ADM\" (or \"download\") in your app launcher."

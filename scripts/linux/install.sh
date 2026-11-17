#!/usr/bin/env sh
# Install Lary from this folder for the current user: the program in
# ~/.local/bin, its icon and its launcher entry. Run it again to update by
# hand; installed copies also update themselves. Uninstall with --remove.
#
#   ./install.sh            # install or update
#   ./install.sh --remove   # uninstall (your sessions and settings stay)
set -eu
here=$(cd "$(dirname "$0")" && pwd)
bin_dir="${XDG_BIN_HOME:-$HOME/.local/bin}"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
apps="$data/applications"
icons="$data/icons/hicolor/256x256/apps"

if [ "${1:-}" = "--remove" ]; then
    rm -f "$bin_dir/lary" "$apps/lary.desktop" "$icons/lary.png"
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$apps" >/dev/null 2>&1 || true
    echo "Lary is uninstalled. Your sessions and settings are in $data/lary."
    exit 0
fi

mkdir -p "$bin_dir" "$apps" "$icons"
install -m 755 "$here/lary" "$bin_dir/lary"
install -m 644 "$here/lary.png" "$icons/lary.png"
sed "s|^Exec=lary|Exec=$bin_dir/lary|" "$here/lary.desktop" > "$apps/lary.desktop"
chmod 644 "$apps/lary.desktop"
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$apps" >/dev/null 2>&1 || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q "$data/icons/hicolor" >/dev/null 2>&1 || true

echo "Lary is installed: $bin_dir/lary, and in your applications menu."
case ":$PATH:" in *":$bin_dir:"*) ;; *) echo "Add $bin_dir to your PATH to start it as 'lary' from a terminal." ;; esac
if ! ldconfig -p 2>/dev/null | grep -q libwebkit2gtk-4.1; then
    echo
    echo "Lary needs WebKitGTK 4.1:"
    echo "  Debian / Ubuntu:  sudo apt install libwebkit2gtk-4.1-0 libgtk-3-0"
    echo "  Fedora:           sudo dnf install webkit2gtk4.1 gtk3"
    echo "  Arch:             sudo pacman -S webkit2gtk-4.1 gtk3"
fi

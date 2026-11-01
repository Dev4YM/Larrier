#!/usr/bin/env sh
# Install Larrier from this folder for the current user: the program in
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
    rm -f "$bin_dir/larrier" "$apps/larrier.desktop" "$icons/larrier.png"
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$apps" >/dev/null 2>&1 || true
    echo "Larrier is uninstalled. Your sessions and settings are in $data/larrier."
    exit 0
fi

mkdir -p "$bin_dir" "$apps" "$icons"
install -m 755 "$here/larrier" "$bin_dir/larrier"
install -m 644 "$here/larrier.png" "$icons/larrier.png"
sed "s|^Exec=larrier|Exec=$bin_dir/larrier|" "$here/larrier.desktop" > "$apps/larrier.desktop"
chmod 644 "$apps/larrier.desktop"
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$apps" >/dev/null 2>&1 || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q "$data/icons/hicolor" >/dev/null 2>&1 || true

echo "Larrier is installed: $bin_dir/larrier, and in your applications menu."
case ":$PATH:" in *":$bin_dir:"*) ;; *) echo "Add $bin_dir to your PATH to start it as 'larrier' from a terminal." ;; esac
if ! ldconfig -p 2>/dev/null | grep -q libwebkit2gtk-4.1; then
    echo
    echo "Larrier needs WebKitGTK 4.1:"
    echo "  Debian / Ubuntu:  sudo apt install libwebkit2gtk-4.1-0 libgtk-3-0"
    echo "  Fedora:           sudo dnf install webkit2gtk4.1 gtk3"
    echo "  Arch:             sudo pacman -S webkit2gtk-4.1 gtk3"
fi

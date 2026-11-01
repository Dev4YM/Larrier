#!/usr/bin/env bash
# Check that this Mac or Linux machine has what developing Larrier needs,
# and say how to get what's missing. Changes nothing.
#
#   ./scripts/setup.sh
set -u
cd "$(dirname "$0")/.."
missing=0

ok()   { printf '  ok   %-18s %s\n' "$1" "$2"; }
opt()  { printf '  --   %-18s %s\n' "$1" "$2"; }
bad()  { printf '  no   %-18s %s\n' "$1" "$2"; missing=$((missing + 1)); }
have() { command -v "$1" >/dev/null 2>&1; }

echo "Required"
if have rustc; then
    v=$(rustc --version)
    minor=$(echo "$v" | sed -E 's/^rustc 1\.([0-9]+).*/\1/')
    if [ "${minor:-0}" -ge 85 ] 2>/dev/null; then ok "Rust 1.85+" "$v"; else bad "Rust 1.85+" "rustup update stable ($v)"; fi
else
    bad "Rust 1.85+" "Install from https://rustup.rs"
fi
have cargo && ok "Cargo" "$(cargo --version)" || bad "Cargo" "Comes with Rust (https://rustup.rs)"
have git && ok "Git" "$(git --version)" || bad "Git" "Install git with your package manager"

case "$(uname -s)" in
    Darwin)
        if xcode-select -p >/dev/null 2>&1; then ok "Xcode tools" "$(xcode-select -p)"; else bad "Xcode tools" "xcode-select --install"; fi
        ;;
    Linux)
        have pkg-config && ok "pkg-config" "$(pkg-config --version)" || bad "pkg-config" "see the packages below"
        for lib in webkit2gtk-4.1 gtk+-3.0 libsoup-3.0; do
            if pkg-config --exists "$lib" 2>/dev/null; then ok "$lib" "$(pkg-config --modversion "$lib")"; else bad "$lib" "missing"; fi
        done
        if [ "$missing" -gt 0 ]; then
            echo
            echo "  Debian / Ubuntu:  sudo apt install build-essential pkg-config libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev"
            echo "  Fedora:           sudo dnf install gcc pkgconf-pkg-config webkit2gtk4.1-devel gtk3-devel libsoup3-devel libappindicator-gtk3-devel librsvg2-devel libxdo-devel openssl-devel"
            echo "  Arch:             sudo pacman -S base-devel webkit2gtk-4.1 gtk3 libsoup3 libappindicator-gtk3 librsvg xdotool openssl"
        fi
        ;;
esac

echo
echo "Optional"
have python3 && ok "Python 3" "$(python3 --version) (scripts/demo)" || opt "Python 3" "for scripts/demo"
if have python3 && python3 -c "import PIL, numpy" 2>/dev/null; then ok "Pillow + NumPy" "for compose_hero.py"; else opt "Pillow + NumPy" "pip install pillow numpy"; fi
have gh && ok "GitHub CLI" "$(gh --version | head -1)" || opt "GitHub CLI" "for releases and pull requests: https://cli.github.com"
have docker && ok "Docker" "$(docker --version) (build.sh --docker)" || opt "Docker" "for build.sh --docker (portable Linux builds)"

echo
if [ "$missing" -gt 0 ]; then echo "$missing required tool(s) missing."; exit 1; fi
echo "Ready. Try: ./scripts/dev.sh --sandbox"

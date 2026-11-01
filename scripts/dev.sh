#!/usr/bin/env bash
# Run the Larrier desktop app from source (macOS and Linux; dev.ps1 on
# Windows).
#
#   ./scripts/dev.sh                        # debug build, your own data
#   ./scripts/dev.sh --sandbox              # a fresh, empty data folder
#   ./scripts/dev.sh --data-dir DIR         # this data folder (LARRIER_HOME)
#   ./scripts/dev.sh --release -- --version # optimized; args after -- go to the app
set -eu
cd "$(dirname "$0")/.."
cargo=(run -p larrier-app)
while [ $# -gt 0 ]; do
    case "$1" in
        --release) cargo+=(--release) ;;
        --sandbox)
            export LARRIER_HOME="${TMPDIR:-/tmp}/larrier-sandbox-$(date +%Y%m%d-%H%M%S)"
            mkdir -p "$LARRIER_HOME"
            echo "Sandbox data folder: $LARRIER_HOME" ;;
        --data-dir) shift; export LARRIER_HOME="$(cd "$1" && pwd)" ;;
        --) shift; cargo+=(-- "$@"); break ;;
        -h|--help) sed -n '2,9p' "$0"; exit 0 ;;
        *) echo "dev.sh: unknown option $1" >&2; exit 2 ;;
    esac
    shift
done
exec cargo "${cargo[@]}"

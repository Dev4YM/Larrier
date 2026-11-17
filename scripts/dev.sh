#!/usr/bin/env bash
# Run the Lary desktop app from source (macOS and Linux; dev.ps1 on
# Windows).
#
#   ./scripts/dev.sh                        # debug build, your own data
#   ./scripts/dev.sh --sandbox              # a fresh, empty data folder
#   ./scripts/dev.sh --data-dir DIR         # this data folder (LARY_HOME)
#   ./scripts/dev.sh --release -- --version # optimized; args after -- go to the app
set -eu
cd "$(dirname "$0")/.."
cargo=(run -p lary-app)
while [ $# -gt 0 ]; do
    case "$1" in
        --release) cargo+=(--release) ;;
        --sandbox)
            export LARY_HOME="${TMPDIR:-/tmp}/lary-sandbox-$(date +%Y%m%d-%H%M%S)"
            mkdir -p "$LARY_HOME"
            echo "Sandbox data folder: $LARY_HOME" ;;
        --data-dir) shift; export LARY_HOME="$(cd "$1" && pwd)" ;;
        --) shift; cargo+=(-- "$@"); break ;;
        -h|--help) sed -n '2,9p' "$0"; exit 0 ;;
        *) echo "dev.sh: unknown option $1" >&2; exit 2 ;;
    esac
    shift
done
exec cargo "${cargo[@]}"

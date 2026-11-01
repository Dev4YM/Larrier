#!/usr/bin/env bash
# Build, test and lint the whole workspace: what a pull request must pass
# (macOS and Linux; check.ps1 on Windows).
#
#   ./scripts/check.sh            # build, tests, Clippy
#   ./scripts/check.sh --quick    # tests only
#   ./scripts/check.sh --ignored  # also the tests that need real agents or the network
set -eu
cd "$(dirname "$0")/.."
quick=0 ignored=0
for a in "$@"; do
    case "$a" in
        --quick) quick=1 ;;
        --ignored) ignored=1 ;;
        *) echo "check.sh: unknown option $a" >&2; exit 2 ;;
    esac
done
step() { local name=$1; shift; echo "==> $name"; cargo "$@" || { echo "check: $name failed" >&2; exit 1; }; }
[ $quick -eq 1 ] || step Build build --workspace --locked
step Test test --workspace --locked
[ $ignored -eq 0 ] || step "Ignored tests" test --workspace --locked -- --ignored
[ $quick -eq 1 ] || step Clippy clippy --workspace --locked
echo "All checks passed."

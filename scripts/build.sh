#!/usr/bin/env bash
# Build Lary's program for this Mac or Linux machine into dist/, packaged
# and named the way releases are, with its SHA-256 (build.ps1 on Windows).
#
#   ./scripts/build.sh                                 # this machine's architecture
#   ./scripts/build.sh --target x86_64-apple-darwin    # macOS: the Intel build on Apple silicon
#   ./scripts/build.sh --docker                        # Linux: in a clean Ubuntu 22.04 container
#
#   macOS → dist/Lary-<version>-macos-<arch>.zip      (Lary.app)
#   Linux → dist/Lary-<version>-linux-<arch>.tar.gz   (lary, lary.desktop, lary.png, install.sh)
set -eu
cd "$(dirname "$0")/.."
target="" docker=0
while [ $# -gt 0 ]; do
    case "$1" in
        --target) shift; target=$1 ;;
        --docker) docker=1 ;;
        -h|--help) sed -n '2,10p' "$0"; exit 0 ;;
        *) echo "build.sh: unknown option $1" >&2; exit 2 ;;
    esac
    shift
done

version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version *= *"\(.*\)"/\1/p' Cargo.toml)
commit=$(git rev-parse --short=9 HEAD 2>/dev/null || echo unknown)
date=$(git log -1 --format=%cs 2>/dev/null || true)

if [ $docker -eq 1 ]; then
    docker build -q -t lary-linux-build -f scripts/linux/Dockerfile scripts/linux >/dev/null
    exec docker run --rm -v "$PWD:/src" -v lary-linux-target:/src/target -v lary-cargo-registry:/root/.cargo/registry \
        -e LARY_COMMIT="$commit" -e LARY_COMMIT_DATE="$date" lary-linux-build ./scripts/build.sh ${target:+--target "$target"}
fi

os=$(uname -s)
machine=${target%%-*}
[ -n "$machine" ] || machine=$(uname -m)
case "$machine" in
    x86_64|amd64) arch=x64 ;;
    aarch64|arm64) arch=arm64 ;;
    *) echo "build.sh: unsupported architecture $machine" >&2; exit 1 ;;
esac

# The oldest macOS the app says it runs on (Info.plist below).
if [ "$os" = Darwin ]; then export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-11.0}"; fi
cargo build --release --locked -p lary-app ${target:+--target "$target"}
bin="target/${target:+$target/}release/lary-desktop"
mkdir -p dist
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT

sha() { if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1; }

case "$os" in
    Darwin)
        name="Lary-$version-macos-$arch.zip"
        app="$stage/Lary.app"
        mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
        cp "$bin" "$app/Contents/MacOS/Lary"
        cp app/assets/brand/lary.icns "$app/Contents/Resources/Lary.icns"
        plist="$app/Contents/Info.plist"
        plutil -create xml1 "$plist"
        for kv in CFBundleName=Lary CFBundleDisplayName=Lary CFBundleIdentifier=com.dev4ym.lary \
                  CFBundleExecutable=Lary CFBundleIconFile=Lary CFBundlePackageType=APPL \
                  CFBundleShortVersionString="$version" CFBundleVersion="$version" \
                  LSMinimumSystemVersion=11.0 LSApplicationCategoryType=public.app-category.developer-tools; do
            plutil -insert "${kv%%=*}" -string "${kv#*=}" "$plist"
        done
        plutil -insert NSHighResolutionCapable -bool true "$plist"
        # Not notarized: signed ad hoc so Apple silicon runs it.
        codesign --force --deep --sign - "$app"
        # Lary was called Larrier: an old Larrier's updater looks for
        # Larrier.app/Contents/MacOS/Larrier in the download. A bare folder
        # beside the (signed, untouched) Lary.app points it at Lary, which
        # then installs itself as Lary.app. Drop once Larrier is gone.
        mkdir -p "$stage/Larrier.app/Contents/MacOS"
        ln -sf ../../../Lary.app/Contents/MacOS/Lary "$stage/Larrier.app/Contents/MacOS/Larrier"
        rm -f "dist/$name"
        ditto -c -k "$stage" "dist/$name"
        ;;
    Linux)
        name="Lary-$version-linux-$arch.tar.gz"
        pkg="$stage/lary"
        mkdir -p "$pkg"
        install -m 755 "$bin" "$pkg/lary"
        install -m 644 app/assets/brand/lary-256.png "$pkg/lary.png"
        install -m 644 scripts/linux/lary.desktop "$pkg/lary.desktop"
        install -m 755 scripts/linux/install.sh "$pkg/install.sh"
        install -m 644 LICENSE "$pkg/LICENSE"
        # Lary was called Larrier: an old Larrier's updater looks for
        # larrier/larrier in the download; links point it at Lary, which
        # then installs itself as `lary`. Drop once Larrier is gone.
        ln -sf lary "$pkg/larrier"
        ln -sf lary "$stage/larrier"
        tar -C "$stage" -czf "dist/$name" lary larrier
        ;;
    *) echo "build.sh: use scripts/build.ps1 on Windows" >&2; exit 1 ;;
esac

hash=$(sha "dist/$name")
printf '%s  %s' "$hash" "$name" > "dist/$name.sha256"
# The program must report the version it's released as (when it runs here).
if out=$("$bin" --version 2>/dev/null); then
    case "$out" in *"$version"*) ;; *) echo "build: the program reports '$out', expected $version" >&2; exit 1 ;; esac
fi
echo "dist/$name  ($hash)"

#!/usr/bin/env bash
set -euo pipefail
project=$(cd "$(dirname "$0")/../.." && pwd)
cd "$project"
artifact="${1:-${CARGO_TARGET_DIR:-src-tauri/target}/release/ttp-desktop}"
if [[ ! -x $artifact ]]; then
    echo "Build the release binary first with npm run tauri -- build." >&2
    exit 1
fi
staging=$(mktemp -d)
trap 'rm -rf -- "$staging"' EXIT
install -m755 "$artifact" "$staging/ttp-desktop"
install -m644 packaging/arch/{PKGBUILD,io.ttp.TTPDesktop.desktop} "$staging"
install -m644 src-tauri/icons/[0-9]*x[0-9]*.png "$staging"
install -m644 node_modules/@fontsource-variable/inter/LICENSE "$staging/Inter-OFL.txt"
install -m644 node_modules/@fontsource-variable/jetbrains-mono/LICENSE "$staging/JetBrainsMono-OFL.txt"
cd "$staging"
makepkg --geninteg > checksums
cat checksums >> PKGBUILD
PKGDEST="$project" PKGEXT=.pkg.tar.zst makepkg --force --clean

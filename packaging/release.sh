#!/usr/bin/env bash
# Builds the files for a GitHub release into dist/:
#   adm-<version>-linux-x86_64.tar.gz   the app, the command-line tool, an installer, icons, license
#   adm-extension-<extension version>.zip   the browser extension
#   SHA256SUMS
# The version is read from src-tauri/Cargo.toml. Usage: packaging/release.sh [--build]   (--build compiles first)
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

if [ "${1:-}" = "--build" ]; then
  nice -n 19 cargo build --release --locked -p adm -p adm-cli -j 2
fi
version="$(sed -n 's/^version = "\(.*\)"/\1/p' src-tauri/Cargo.toml | head -1)"
name="adm-$version-linux-x86_64"
[ -x target/release/adm ] && [ -x target/release/adm-dl ] || { echo "Build first: packaging/release.sh --build" >&2; exit 1; }

rm -rf "dist/$name" "dist/$name.tar.gz"
mkdir -p "dist/$name/icons"
cp target/release/adm target/release/adm-dl "dist/$name/"
cp src-tauri/icons/128x128.png "dist/$name/icons/128.png"
cp src-tauri/icons/icon.png "dist/$name/icons/512.png"
cp packaging/install.sh.in "dist/$name/install.sh" && chmod +x "dist/$name/install.sh"
cp LICENSE NOTICE "dist/$name/"
cat > "dist/$name/README.txt" <<TXT
ADM $version

Install (no root needed):   ./install.sh
Remove:                     ./install.sh --uninstall

ADM needs WebKitGTK 4.1 (Arch: webkit2gtk-4.1, Debian/Ubuntu: libwebkit2gtk-4.1-0, Fedora: webkit2gtk4.1).
Project page and documentation: https://github.com/Axenith/ADM
TXT
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime='2026-01-01' -czf "dist/$name.tar.gz" -C dist "$name"
rm -rf "dist/$name"

python3 packaging/build-extension.py
(cd dist && sha256sum "$name.tar.gz" adm-extension-*.zip > SHA256SUMS && cat SHA256SUMS)
echo; ls -l dist | awk 'NR>1 {print $5, $NF}'

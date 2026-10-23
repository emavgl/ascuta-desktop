#!/usr/bin/env bash
# Creates dist/ascuta-desktop-<version>-linux-x86_64.tar.gz with the binary,
# the sherpa-onnx libraries and install.sh.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

VERSION="${1:-v$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')}"
PKG="ascuta-desktop-${VERSION}-linux-x86_64"

cargo build --release --locked --features "capture sherpa"

rm -rf "dist/${PKG}"
mkdir -p "dist/${PKG}"
cp target/release/ascuta-desktop "dist/${PKG}/"
strip "dist/${PKG}/ascuta-desktop" || true
for so in libsherpa-onnx-c-api.so libonnxruntime.so; do
  cp "target/release/${so}" "dist/${PKG}/"
done
cp packaging/install.sh "dist/${PKG}/install.sh"
cp README.md "dist/${PKG}/README.md"
chmod +x "dist/${PKG}/install.sh"

tar -C dist -czf "dist/${PKG}.tar.gz" "${PKG}"
( cd dist && sha256sum "${PKG}.tar.gz" > "${PKG}.tar.gz.sha256" )

# Stable names used by the public website release, so install.sh can point
# at a download URL that never changes.
cp "dist/${PKG}.tar.gz" "dist/ascuta-desktop-linux-x86_64.tar.gz"
( cd dist && sha256sum "ascuta-desktop-linux-x86_64.tar.gz" > "ascuta-desktop-linux-x86_64.tar.gz.sha256" )
cp packaging/install.sh "dist/install.sh"
chmod +x "dist/install.sh"

ls -lh "dist/${PKG}.tar.gz" "dist/ascuta-desktop-linux-x86_64.tar.gz"

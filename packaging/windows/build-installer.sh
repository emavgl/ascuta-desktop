#!/usr/bin/env bash
# Builds the single-file Windows installer (NSIS).
#   packaging/windows/build-installer.sh [version]
# Needs: makensis (NSIS 3) and the unpacked package in dist/ (see build-zip.ps1
# or the release zip). Output: dist/ascuta-desktop-<version>-windows-x86_64-setup.exe
set -euo pipefail
cd "$(dirname "$0")/../.."

VERSION="${1:-v$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')}"
NUM="${VERSION#v}"
PKG="ascuta-desktop-${VERSION}-windows-x86_64"
SRC="dist/${PKG}"
OUT="dist/${PKG}-setup.exe"
STABLE="dist/ascuta-desktop-windows-x86_64-setup.exe"

if [ ! -f "$SRC/ascuta-desktop.exe" ]; then
  printf 'error: %s not found; build the Windows package first\n' "$SRC" >&2
  exit 1
fi

cp packaging/windows/app.ico "$SRC/app.ico"
MAKENSIS="${MAKENSIS:-makensis}"
command -v "$MAKENSIS" >/dev/null 2>&1 || {
  printf 'error: %s not found (install NSIS 3 or set MAKENSIS)\n' "$MAKENSIS" >&2
  exit 1
}

ROOT="$(pwd)"
(
  cd packaging/windows
  "$MAKENSIS" -V2 \
    "-DVERSION=${NUM}" \
    "-DSOURCE_DIR=${ROOT}/${SRC}" \
    "-DOUTFILE=${ROOT}/${OUT}" \
    installer.nsi
)

cp "$OUT" "$STABLE"
( cd dist && sha256sum "${PKG}-setup.exe" > "${PKG}-setup.exe.sha256" )
( cd dist && sha256sum "ascuta-desktop-windows-x86_64-setup.exe" > "ascuta-desktop-windows-x86_64-setup.exe.sha256" )

ls -lh "$OUT" "$STABLE" "$OUT.sha256" "$STABLE.sha256"

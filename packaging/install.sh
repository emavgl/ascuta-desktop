#!/usr/bin/env bash
set -euo pipefail

PREFIX="${PREFIX:-$HOME/.local}"
LIB="$PREFIX/lib/ascuta"
BIN="$PREFIX/bin"
APPS="$PREFIX/share/applications"
AUTOSTART="${XDG_CONFIG_HOME:-$HOME/.config}/autostart/ascuta-desktop.desktop"
DATA="${XDG_DATA_HOME:-$HOME/.local/share}/ascuta"
MODELS="${ASCUTA_MODELS_DIR:-$DATA/models}"
SETTINGS="${ASCUTA_CONFIG_FILE:-${XDG_CONFIG_HOME:-$HOME/.config}/ascuta/settings.json}"

RELEASES_URL="${ASCUTA_RELEASES_URL:-https://github.com/emavgl/ascuta-desktop/releases/latest/download}"
ASSET="${ASCUTA_ASSET:-ascuta-desktop-linux-x86_64.tar.gz}"

ASSUME_YES=0
PURGE=0
FROM_URL=""
ACTION="install"
SKIP_VERIFY="${ASCUTA_SKIP_VERIFY:-0}"
TMPDIRS=()

log() { printf '%s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }
cleanup() {
  local d
  for d in "${TMPDIRS[@]}"; do
    if [ -n "$d" ]; then rm -rf "$d"; fi
  done
}
trap cleanup EXIT

usage() {
  cat <<'EOF'
Usage: ./install.sh [options]

Installs the prebuilt Ascuta Desktop binary for the current user. It
installs the required system libraries, downloads the release package
(binary + shared libraries, no source code), verifies its checksum and
installs it. If the script sits next to `ascuta-desktop` and the `.so`
libraries (extracted package), it uses those instead.

Options:
  -y, --yes         do not ask for confirmation
      --from URL    download the .tar.gz package from URL and install it
      --uninstall   remove Ascuta Desktop
      --purge       with --uninstall, also remove settings and models
  -h, --help        show this help

Environment: PREFIX, ASCUTA_MODELS_DIR, ASCUTA_CONFIG_FILE,
ASCUTA_RELEASES_URL (base URL), ASCUTA_ASSET (file name),
ASCUTA_SKIP_VERIFY=1 (do not check the SHA-256).
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    -y|--yes) ASSUME_YES=1 ;;
    --from) [ $# -ge 2 ] || die "--from requires a URL"; FROM_URL="$2"; shift ;;
    --uninstall) ACTION="uninstall" ;;
    --purge) PURGE=1 ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown option: $1 (try --help)" ;;
  esac
  shift
done

as_root() {
  if [ "$(id -u)" -eq 0 ]; then
    "$@"
  elif command -v sudo >/dev/null 2>&1; then
    sudo "$@"
  elif command -v pkexec >/dev/null 2>&1; then
    pkexec "$@"
  else
    return 1
  fi
}

ask() {
  local prompt="$1" answer
  if [ "$ASSUME_YES" -eq 1 ]; then
    return 0
  fi
  if [ ! -t 0 ]; then
    return 1
  fi
  read -r -p "$prompt [Y/n] " answer
  case "${answer:-Y}" in
    n|N|no|NO) return 1 ;;
    *) return 0 ;;
  esac
}

remove_path() {
  [ -e "$1" ] || [ -L "$1" ] || return 0
  rm -f "$1"
}

if [ "$ACTION" = "uninstall" ]; then
  remove_path "$BIN/ascuta-desktop"
  remove_path "$APPS/ascuta-desktop.desktop"
  remove_path "$AUTOSTART"
  rm -rf "$LIB"
  command -v update-desktop-database >/dev/null 2>&1 &&
    update-desktop-database "$APPS" 2>/dev/null || true
  if [ "$PURGE" -eq 1 ]; then
    rm -rf "$DATA" "$(dirname "$SETTINGS")"
    log "Ascuta Desktop, settings and models removed."
  else
    log "Ascuta Desktop removed. Settings and models are kept in:"
    log "  $(dirname "$SETTINGS")"
    log "  $MODELS"
    log "Use --uninstall --purge to delete them."
  fi
  exit 0
fi

[ "$(uname -s)" = "Linux" ] || die "this installer is for Linux only"
case "$(uname -m)" in
  x86_64|amd64) ;;
  *) die "the package is only available for Linux x86_64 (architecture: $(uname -m))" ;;
esac

script="${BASH_SOURCE[0]:-}"
if [ -n "$script" ] && [ -f "$script" ]; then
  here="$(cd "$(dirname "$script")" && pwd)"
else
  here=""
fi
pm=""
if command -v apt-get >/dev/null 2>&1; then pm="apt"
elif command -v dnf >/dev/null 2>&1; then pm="dnf"
elif command -v pacman >/dev/null 2>&1; then pm="pacman"
elif command -v zypper >/dev/null 2>&1; then pm="zypper"
elif command -v apk >/dev/null 2>&1; then pm="apk"
fi

pkg_available() {
  local p="$1"
  case "$pm" in
    apt) apt-cache show "$p" >/dev/null 2>&1 ;;
    dnf) dnf -q list --available "$p" >/dev/null 2>&1 || dnf -q list installed "$p" >/dev/null 2>&1 ;;
    pacman) pacman -Si "$p" >/dev/null 2>&1 || pacman -Qi "$p" >/dev/null 2>&1 ;;
    zypper) zypper -q info "$p" >/dev/null 2>&1 ;;
    apk) apk info -e "$p" >/dev/null 2>&1 || apk search -x "$p" >/dev/null 2>&1 ;;
    *) return 1 ;;
  esac
}

pkg_candidates() {
  case "$pm:$1" in
    apt:libasound.so.2) echo "libasound2t64 libasound2" ;;
    apt:libbz2.so.1.0|apt:libbz2.so.1) echo "libbz2-1.0" ;;
    apt:libstdc++.so.6) echo "libstdc++6" ;;
    apt:libgcc_s.so.1) echo "libgcc-s1 libgcc1" ;;
    apt:libc.so.6) echo "libc6" ;;
    dnf:libasound.so.2) echo "alsa-lib" ;;
    dnf:libbz2.so.1.0|dnf:libbz2.so.1) echo "bzip2-libs" ;;
    dnf:libstdc++.so.6) echo "libstdc++" ;;
    dnf:libgcc_s.so.1) echo "libgcc" ;;
    dnf:libc.so.6) echo "glibc" ;;
    pacman:libasound.so.2) echo "alsa-lib" ;;
    pacman:libbz2.so.1.0|pacman:libbz2.so.1) echo "bzip2" ;;
    pacman:libstdc++.so.6|pacman:libgcc_s.so.1) echo "gcc-libs" ;;
    pacman:libc.so.6) echo "glibc" ;;
    zypper:libasound.so.2) echo "libasound2" ;;
    zypper:libbz2.so.1.0|zypper:libbz2.so.1) echo "libbz2-1" ;;
    zypper:libstdc++.so.6) echo "libstdc++6" ;;
    zypper:libgcc_s.so.1) echo "libgcc_s1" ;;
    zypper:libc.so.6) echo "glibc" ;;
    apk:libasound.so.2) echo "alsa-lib" ;;
    apk:libbz2.so.1.0|apk:libbz2.so.1) echo "bzip2-libs" ;;
    apk:libstdc++.so.6) echo "libstdc++" ;;
    apk:libgcc_s.so.1) echo "libgcc" ;;
    apk:libc.so.6) echo "musl" ;;
    *) echo "" ;;
  esac
}

pick_pkg() {
  local so="$1" p
  for p in $(pkg_candidates "$so"); do
    if pkg_available "$p"; then
      echo "$p"
      return 0
    fi
  done
  return 1
}

missing_libs() {
  local f
  for f in "$LIB"/ascuta-desktop "$LIB"/*.so; do
    [ -e "$f" ] || continue
    ldd "$f" 2>/dev/null | awk '/not found/ { print $1 }'
  done | sort -u || true
}

pm_updated=0
pm_sync() {
  [ "$pm_updated" -eq 1 ] && return 0
  case "$pm" in
    apt) as_root env DEBIAN_FRONTEND=noninteractive apt-get update -qq || true ;;
    pacman) as_root pacman -Sy --noconfirm || true ;;
  esac
  pm_updated=1
}

install_pkgs() {
  [ $# -gt 0 ] || return 0
  pm_sync
  log "Installing packages: $*"
  case "$pm" in
    apt) as_root env DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends "$@" ;;
    dnf) as_root dnf install -y "$@" ;;
    pacman) as_root pacman -S --needed --noconfirm "$@" ;;
    zypper) as_root zypper --non-interactive install "$@" ;;
    apk) as_root apk add "$@" ;;
    *) return 1 ;;
  esac
}

download() {
  local url="$1" dest="$2"
  if command -v curl >/dev/null 2>&1; then
    curl -fL --retry 3 --progress-bar -o "$dest" "$url"
  elif command -v wget >/dev/null 2>&1; then
    wget -O "$dest" "$url"
  else
    die "curl or wget is required to download the package"
  fi
}

verify_checksum() {
  local url="$1" archive="$2" dir sum
  [ "$SKIP_VERIFY" = "1" ] && return 0
  dir="$(dirname "$archive")"
  sum="$dir/$(basename "$archive").sha256"
  if ! download "$url.sha256" "$sum" 2>/dev/null; then
    warn "no checksum available for $(basename "$archive"); skipping verification"
    return 0
  fi
  log "Verifying SHA-256 checksum..."
  ( cd "$dir" && sha256sum -c "$(basename "$sum")" ) ||
    die "checksum verification failed: the download is corrupt or has been tampered with"
}

payload="$here"

if [ ! -x "$payload/ascuta-desktop" ]; then
  if [ -z "$FROM_URL" ] && [ -n "${ASCUTA_PAYLOAD_URL:-}" ]; then
    FROM_URL="$ASCUTA_PAYLOAD_URL"
  fi
  [ -n "$FROM_URL" ] || FROM_URL="$RELEASES_URL/$ASSET"
  tmp="$(mktemp -d)"
  TMPDIRS+=("$tmp")
  archive="$tmp/$(basename "${FROM_URL%%\?*}")"
  log "Downloading Ascuta Desktop from $FROM_URL"
  download "$FROM_URL" "$archive"
  verify_checksum "$FROM_URL" "$archive"
  tar -xzf "$archive" -C "$tmp" || die "could not extract the package"
  bin_found="$(find "$tmp" -maxdepth 2 -type f -name ascuta-desktop | head -n 1)"
  [ -n "$bin_found" ] || die "the archive does not contain ascuta-desktop"
  payload="$(dirname "$bin_found")"
fi

[ -x "$payload/ascuta-desktop" ] || die "ascuta-desktop not found in $payload"

mkdir -p "$LIB" "$BIN" "$APPS"
install -m755 "$payload/ascuta-desktop" "$LIB/ascuta-desktop"
shopt -s nullglob
for so in "$payload"/*.so "$payload"/*.so.*; do
  install -m755 "$so" "$LIB/"
done
shopt -u nullglob
ln -sf "$LIB/ascuta-desktop" "$BIN/ascuta-desktop"

missing="$(missing_libs)"
if [ -n "$missing" ]; then
  [ -n "$pm" ] || die "missing libraries ($(echo "$missing" | tr '\n' ' ')) and no known package manager"
  if ! ask "Install the missing system libraries?"; then
    die "missing libraries: $(echo "$missing" | tr '\n' ' ')"
  fi
  pm_sync
  pkgs=()
  for so in $missing; do
    if pkg="$(pick_pkg "$so")"; then
      case " ${pkgs[*]} " in *" $pkg "*) ;; *) pkgs+=("$pkg") ;; esac
    else
      warn "no known package for $so"
    fi
  done
  install_pkgs "${pkgs[@]}"
  missing="$(missing_libs)"
  [ -z "$missing" ] || die "libraries still missing: $(echo "$missing" | tr '\n' ' ')"
fi

if [ -z "$pm" ]; then
  warn "unknown package manager: skipping optional tools"
else
  extras=()
  if [ "${XDG_SESSION_TYPE:-}" = "wayland" ] || [ -n "${WAYLAND_DISPLAY:-}" ]; then
    command -v wl-paste >/dev/null 2>&1 || extras+=("wl-clipboard")
    if ! command -v xdg-desktop-portal >/dev/null 2>&1 &&
       [ ! -x /usr/libexec/xdg-desktop-portal ] &&
       [ ! -x /usr/lib/xdg-desktop-portal ]; then
      extras+=("xdg-desktop-portal")
      case "${XDG_CURRENT_DESKTOP:-}" in
        *KDE*|*Plasma*) extras+=("xdg-desktop-portal-kde") ;;
        *GNOME*) extras+=("xdg-desktop-portal-gnome") ;;
        *Hyprland*|*sway*|*wlroots*) extras+=("xdg-desktop-portal-wlr") ;;
        *) extras+=("xdg-desktop-portal-gtk") ;;
      esac
    fi
  else
    command -v xclip >/dev/null 2>&1 || extras+=("xclip")
  fi
  kept=()
  if [ ${#extras[@]} -gt 0 ]; then pm_sync; fi
  for p in "${extras[@]}"; do
    pkg_available "$p" && kept+=("$p")
  done
  if [ ${#kept[@]} -gt 0 ] && ask "Also install the optional tools (${kept[*]})?"; then
    install_pkgs "${kept[@]}" || warn "could not install the optional tools"
  fi
fi

profile_line="case \":\$PATH:\" in *\":$BIN:\"*) ;; *) PATH=\"$BIN:\$PATH\" ;; esac"
case ":$PATH:" in
  *":$BIN:"*) ;;
  *)
    for rc in "$HOME/.profile" "$HOME/.bashrc" "$HOME/.zshrc"; do
      [ -f "$rc" ] || continue
      grep -q 'Ascuta Desktop: PATH' "$rc" 2>/dev/null && continue
      {
        printf '\n# Ascuta Desktop: PATH\n'
        printf '%s\n' "$profile_line"
        printf 'export PATH\n'
      } >> "$rc"
    done
    ;;
esac

cat > "$APPS/ascuta-desktop.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Ascuta Desktop
Comment=Private offline dictation
Exec=$LIB/ascuta-desktop --gui
Icon=audio-input-microphone
Terminal=false
Categories=Utility;AudioVideo;
StartupNotify=false
EOF
command -v update-desktop-database >/dev/null 2>&1 &&
  update-desktop-database "$APPS" 2>/dev/null || true

if [ "$(id -u)" -ne 0 ] && ! id -nG "$(id -un)" | tr ' ' '\n' | grep -qx input; then
  log
  log "Ascuta needs to read the keyboard for the global hotkey (input group)."
  if ask "Add $(id -un) to the input group?"; then
    if as_root usermod -aG input "$(id -un)"; then
      log "Done. Log out and back in for the change to take effect."
    else
      warn "could not add the group; you can enable it from Ascuta's settings"
    fi
  else
    log "Skipped: you can enable it from Ascuta's settings."
  fi
fi

if "$LIB/ascuta-desktop" --check >/dev/null 2>&1; then
  log
  log "Self-test passed."
else
  warn "self-test (--check) failed: run $LIB/ascuta-desktop --check for details"
fi

log
log "Ascuta Desktop installed to $LIB"
log "Start it from the application menu (\"Ascuta Desktop\") or with:"
log "  $BIN/ascuta-desktop --gui"
case ":$PATH:" in
  *":$BIN:"*) ;;
  *) log "Reopen the terminal (or log in again) to have \"ascuta-desktop\" in PATH." ;;
esac
log "The speech model is downloaded by the app on first launch."

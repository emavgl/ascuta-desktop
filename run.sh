#!/usr/bin/env bash
# Starts Ascuta Desktop (works with a double click too: no terminal needed).
#   ./run.sh            # opens the settings window (default)
#   ./run.sh --pair     # phone bridge only, QR in the terminal
#   ./run.sh --check    # self-test
# Log: ~/.cache/ascuta/run.log
set -uo pipefail
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:$PATH"
export LD_LIBRARY_PATH="$PWD/target/release:${LD_LIBRARY_PATH:-}"

LOG="$HOME/.cache/ascuta/run.log"
mkdir -p "$(dirname "$LOG")"

notify() {
  command -v notify-send >/dev/null 2>&1 && notify-send -a Ascuta "Ascuta Desktop" "$1" || true
  echo "ascuta: $1" >&2
}

ARGS=("$@")
[ ${#ARGS[@]} -eq 0 ] && ARGS=(--gui)

FEATURES=""
pkg-config --exists alsa 2>/dev/null && FEATURES="capture sherpa"

{
  echo "=== $(date) run.sh ${ARGS[*]} (features: ${FEATURES:-none}) ==="
  if [ -n "$FEATURES" ]; then
    cargo build --release --features "$FEATURES"
  else
    cargo build --release
  fi
} >>"$LOG" 2>&1 || { notify "Build failed. Details: $LOG"; exit 1; }

BIN="target/release/ascuta-desktop"
[ -x "$BIN" ] || { notify "Binary not found ($BIN). See $LOG"; exit 1; }

# Single instance: stop the previous one (same ports, same hotkey).
pkill -x ascuta-desktop 2>/dev/null && sleep 1

if [ -n "$FEATURES" ]; then :; else
  echo "ascuta: local dictation disabled (libasound2-dev missing)" >>"$LOG"
fi

case "${ARGS[0]}" in
  --pair|--check)
    exec "$BIN" "${ARGS[@]}" ;;
  *)
    nohup "$BIN" "${ARGS[@]}" >>"$LOG" 2>&1 &
    sleep 2
    if ! pgrep -x ascuta-desktop >/dev/null; then
      notify "Ascuta exited immediately. See $LOG"
      exit 1
    fi
    notify "Ascuta started (tray icon / settings window)." ;;
esac

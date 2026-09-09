#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
destination="${1:-$root/docs/screenshot.png}"

readonly MCP_PORT=8080
readonly APP_NAME="u2dm"
readonly FEATURES="demo slint/mcp"
readonly PROFILE="inspect"
readonly ROOM_ROW=${ROOM_ROW:-0}
readonly READY_TIMEOUT=60
readonly RENDER_SETTLE=3

export SLINT_EMIT_DEBUG_INFO=1

source "$root/scripts/lib/mcp.sh"

tmp="$(mktemp -d)"
app_pid=""
log="$tmp/app.log"

cleanup() {
  if [[ -n $app_pid ]]; then
    kill -- -"$app_pid" 2>/dev/null || kill "$app_pid" 2>/dev/null || true
  fi
  pkill -x "$APP_NAME" 2>/dev/null || true
  rm -rf "$tmp"
  return 0
}
trap cleanup EXIT

require_tools() {
  local tool
  for tool in cargo curl jq base64; do
    command -v "$tool" >/dev/null || { echo "$tool is required" >&2; exit 1; }
  done
  [[ -n ${DISPLAY:-}${WAYLAND_DISPLAY:-} ]] || { echo "a graphical session is required, the window has to render somewhere" >&2; exit 1; }
}

build_demo_app() {
  echo "building the demo app with the Slint inspector"
  cargo build --profile "$PROFILE" --features "$FEATURES"
}

launch_demo_app() {
  pkill -x "$APP_NAME" 2>/dev/null && sleep 1
  setsid env SLINT_MCP_PORT=$MCP_PORT cargo run --profile "$PROFILE" --features "$FEATURES" >"$log" 2>&1 &
  app_pid=$!
  echo "launched the demo app, inspector on $MCP_URL"
}

give_up() {
  echo "$1" >&2
  cat "$log" >&2
  exit 1
}

mcp_fail() { give_up "$1"; }

await() {
  local description=$1 waited=0 result
  shift
  while ((waited < READY_TIMEOUT)); do
    kill -0 "$app_pid" 2>/dev/null || give_up "the app exited before $description"
    result=$("$@" 2>/dev/null) || result=""
    if [[ -n $result && $result != "[]" ]]; then
      echo "$result"
      return
    fi
    sleep 1
    ((waited += 1))
  done
  give_up "timed out waiting for $description"
}

open_room() {
  local room
  room=$(jq -c ".[$ROOM_ROW] // empty" <<<"$1")
  [[ -n $room ]] || give_up "the sidebar has no room at row $ROOM_ROW"
  mcp_click "$room"
  sleep "$RENDER_SETTLE"
}

capture() {
  mcp_screenshot "$1" "$destination"
  echo "wrote $destination"
}

require_tools
build_demo_app
launch_demo_app
window=$(await "the inspector to come up" mcp_window)
root=$(await "the window to report its root element" mcp_root_element "$window")
rooms=$(await "the demo rooms to load" mcp_elements_of_type "$root" RoomRow)
open_room "$rooms"
capture "$window"

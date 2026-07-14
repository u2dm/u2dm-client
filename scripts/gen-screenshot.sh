#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
destination="${1:-$root/docs/screenshot.png}"

readonly MCP_PORT=8080
readonly MCP_URL="http://127.0.0.1:$MCP_PORT/mcp"
readonly APP_NAME="u2dm"
readonly FEATURES="demo slint/mcp"
readonly ROOM_ROW=${ROOM_ROW:-0}
readonly READY_TIMEOUT=60
readonly RENDER_SETTLE=3

export SLINT_EMIT_DEBUG_INFO=1

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
  cargo build --features "$FEATURES"
}

launch_demo_app() {
  pkill -x "$APP_NAME" 2>/dev/null && sleep 1
  setsid env SLINT_MCP_PORT=$MCP_PORT cargo run --features "$FEATURES" >"$log" 2>&1 &
  app_pid=$!
  echo "launched the demo app, inspector on $MCP_URL"
}

give_up() {
  echo "$1" >&2
  cat "$log" >&2
  exit 1
}

call_tool() {
  local tool=$1 arguments=$2 request response
  request=$(jq -cn --arg tool "$tool" --argjson arguments "$arguments" \
    '{jsonrpc: "2.0", id: 1, method: "tools/call", params: {name: $tool, arguments: $arguments}}')
  response=$(curl -sf --max-time 30 -H 'Content-Type: application/json' -d "$request" "$MCP_URL") || return 1
  [[ $(jq -r '.result.isError // false' <<<"$response") == false ]] || give_up "$tool failed: $(jq -r '.result.content[0].text' <<<"$response")"
  echo "$response"
}

tool_payload() {
  call_tool "$1" "$2" | jq -c '.result.content[0].text | fromjson'
}

window_handle() {
  tool_payload list_windows '{}' | jq -c '.windowHandles[0] // empty'
}

room_rows() {
  tool_payload find_elements_by_id \
    "$(jq -cn --argjson window "$1" '{windowHandle: $window, elementsId: "Sidebar::touch"}')" |
    jq -c '.elementHandles // []'
}

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
  call_tool click_element "$(jq -cn --argjson room "$room" '{elementHandle: $room}')" >/dev/null
  sleep "$RENDER_SETTLE"
}

capture() {
  mkdir -p "$(dirname "$destination")"
  call_tool take_screenshot "$(jq -cn --argjson window "$1" '{windowHandle: $window}')" |
    jq -r '.result.content[] | select(.type == "image") | .data' |
    base64 -d >"$destination"
  [[ -s $destination ]] || give_up "the inspector returned an empty screenshot"
  echo "wrote $destination"
}

require_tools
build_demo_app
launch_demo_app
window=$(await "the inspector to come up" window_handle)
rooms=$(await "the demo rooms to load" room_rows "$window")
open_room "$rooms"
capture "$window"

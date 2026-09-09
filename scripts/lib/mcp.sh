: "${MCP_PORT:=8080}"
: "${MCP_URL:=http://127.0.0.1:$MCP_PORT/mcp}"

mcp_fail() {
  echo "$1" >&2
  exit 1
}

mcp_call() {
  local tool=$1 arguments=$2 request response
  request=$(jq -cn --arg tool "$tool" --argjson arguments "$arguments" \
    '{jsonrpc: "2.0", id: 1, method: "tools/call", params: {name: $tool, arguments: $arguments}}')
  response=$(curl -sf --max-time 30 -H 'Content-Type: application/json' -d "$request" "$MCP_URL") || return 1
  [[ $(jq -r '.result.isError // false' <<<"$response") == false ]] ||
    mcp_fail "$tool failed: $(jq -r '.result.content[0].text' <<<"$response")"
  echo "$response"
}

mcp_payload() {
  mcp_call "$1" "$2" | jq -c '.result.content[0].text | fromjson'
}

mcp_window() {
  mcp_payload list_windows '{}' | jq -c '.windowHandles[0] // empty'
}

mcp_root_element() {
  mcp_payload get_window_properties \
    "$(jq -cn --argjson window "$1" '{windowHandle: $window}')" |
    jq -c '.rootElementHandle // empty'
}

mcp_elements_of_type() {
  mcp_payload query_element_descendants \
    "$(jq -cn --argjson root "$1" --arg type "$2" \
      '{elementHandle: $root, findAll: true, queryStack: [{matchElementTypeNameOrBase: $type}]}')" |
    jq -c '.elementHandles // []'
}

mcp_click() {
  mcp_call click_element "$(jq -cn --argjson element "$1" '{elementHandle: $element}')" >/dev/null
}

mcp_screenshot() {
  local window=$1 destination=$2
  mkdir -p "$(dirname "$destination")"
  mcp_call take_screenshot "$(jq -cn --argjson window "$window" '{windowHandle: $window}')" |
    jq -r '.result.content[] | select(.type == "image") | .data' |
    base64 -d >"$destination"
  [[ -s $destination ]] || mcp_fail "the inspector returned an empty screenshot"
}

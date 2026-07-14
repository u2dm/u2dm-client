#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
assets="$root/assets/demo"
data="$assets/data.json"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
failures="$tmp/failures"
: >"$failures"

refetch_existing=0
[[ ${1:-} == "--force" ]] && refetch_existing=1

readonly AVATAR_SIZE=256
readonly SPACE_TILE_SIZE=192
readonly PASTEL_DARKEST=160
readonly PASTEL_SPREAD=80
readonly CC0_AVATAR_STYLES=(open-peeps notionists lorelei pixel-art)
readonly CC0_SPACE_STYLES=(shapes rings glass identicon)
readonly DICEBEAR="https://api.dicebear.com/9.x"
readonly PHOTOS="https://picsum.photos/seed"

require_tools() {
  local tool
  for tool in curl jq magick; do
    command -v "$tool" >/dev/null || { echo "$tool is required" >&2; exit 1; }
  done
  [[ -f $data ]] || { echo "$data not found, nothing to derive images from" >&2; exit 1; }
}

url_encode() {
  jq -rn --arg value "$1" '$value|@uri'
}

matrix_localpart() {
  local user_id=${1#@}
  echo "${user_id%%:*}"
}

digest_of() {
  printf '%s' "$1" | sha256sum
}

pastel_color_for() {
  local digest color="" pair
  digest=$(digest_of "$1")
  for pair in "${digest:0:2}" "${digest:10:2}" "${digest:20:2}"; do
    color+=$(printf '%02x' $((PASTEL_DARKEST + 16#$pair % PASTEL_SPREAD)))
  done
  echo "$color"
}

style_for() {
  local -n styles=$2
  local digest index
  digest=$(digest_of "$1")
  index=$((16#${digest:30:2} % ${#styles[@]}))
  echo "${styles[index]}"
}

avatar_style_for() {
  style_for "$1" CC0_AVATAR_STYLES
}

space_style_for() {
  style_for "$1" CC0_SPACE_STYLES
}

avatar_url() {
  echo "$DICEBEAR/$(avatar_style_for "$1")/png?seed=$(url_encode "$1")&size=$AVATAR_SIZE&radius=50&backgroundType=solid&backgroundColor=$(pastel_color_for "$1")"
}

space_tile_url() {
  echo "$DICEBEAR/$(space_style_for "$1")/png?seed=$(url_encode "$1")&size=$SPACE_TILE_SIZE&radius=50&backgroundType=solid&backgroundColor=$(pastel_color_for "$1")"
}

photo_url() {
  echo "$PHOTOS/$(url_encode "$1")/$2/$3"
}

user_ids() {
  jq -r '[.session.user_id] + [.rooms[].last_message.sender_id // empty]
         + [.timelines[][].sender] | unique | .[]' "$data"
}

space_avatars() {
  jq -r '.spaces[] | select(.avatar) | .avatar' "$data"
}

photo_messages() {
  jq -r '.timelines[][] | select(.image) | "\(.id) \(.image.width) \(.image.height)"' "$data"
}

already_fetched() {
  [[ -f $1 && $refetch_existing -eq 0 ]]
}

download_to() {
  local url=$1 destination=$2
  if curl -sfL --max-time 20 "$url" -o "$tmp/download"; then
    magick "$tmp/download" "$destination"
    echo "  $(basename "$destination")"
  else
    echo "  $(basename "$destination") FAILED" >&2
    echo "$destination" >>"$failures"
  fi
}

fetch() {
  local url=$1 destination=$2
  if already_fetched "$destination"; then
    echo "  $(basename "$destination") (kept, --force to refetch)"
  else
    download_to "$url" "$destination"
  fi
}

fetch_avatars() {
  local user_id
  while read -r user_id; do
    fetch "$(avatar_url "$user_id")" "$assets/avatar-$(matrix_localpart "$user_id").png"
  done < <(user_ids)
}

fetch_space_tiles() {
  local space
  while read -r space; do
    fetch "$(space_tile_url "$space")" "$assets/space-$space.png"
  done < <(space_avatars)
}

fetch_photos() {
  local id width height
  while read -r id width height; do
    fetch "$(photo_url "$id" "$width" "$height")" "$assets/thumbnail-$id.png"
  done < <(photo_messages)
}

report() {
  local failed
  failed=$(wc -l <"$failures")
  if [[ $failed -gt 0 ]]; then
    echo "$failed image(s) could not be fetched, the demo falls back to initials for those" >&2
  fi
  echo "done"
}

require_tools
echo "fetching demo images into $assets"
fetch_avatars
fetch_space_tiles
fetch_photos
report

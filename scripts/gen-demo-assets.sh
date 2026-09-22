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
readonly STICKER_SIZE=512
readonly PASTEL_DARKEST=160
readonly PASTEL_SPREAD=80
readonly CC0_AVATAR_STYLES=(open-peeps notionists lorelei pixel-art)
readonly CC0_SPACE_STYLES=(shapes rings glass identicon)
readonly DICEBEAR="https://api.dicebear.com/9.x"
readonly PHOTOS="https://picsum.photos/seed"
readonly HEIF_BRAND_BOX='\x00\x00\x00\x18ftypheic\x00\x00\x00\x00mif1heic'
readonly UNDECODABLE_SEED_SIZE=64x48
readonly JPEG_FRAME_HEADER_TO_HEIGHT=5

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
         + [.rooms[].avatar // empty | select(startswith("@"))]
         + [.timelines[][].sender] | unique | .[]' "$data"
}

room_avatars() {
  jq -r '(.rooms[], (.unjoined[]? | select(.space | not))) | .avatar // empty
         | select(startswith("@") | not)' "$data"
}

space_avatars() {
  jq -r '(.spaces[], (.unjoined[]? | select(.space))) | select(.avatar) | .avatar' "$data"
}

photo_messages() {
  jq -r '.timelines[][] | select(.image) | "\(.id) \(.image.width) \(.image.height)"' "$data"
}

video_messages() {
  jq -r '.timelines[][] | select(.video) | "\(.id) \(.video.width) \(.video.height)"' "$data"
}

video_clips() {
  jq -r '.timelines[][] | select(.video)
    | "\(.id) \(.video.width) \(.video.height) \([.video.duration_secs // 5, 30] | min)"' "$data"
}

audio_clips() {
  jq -r '.timelines[][] | select(.audio)
    | "\(.id) \(.audio.kind) \([.audio.duration_secs // 5, 30] | min)"' "$data"
}

sticker_messages() {
  jq -r '.timelines[][] | select(.sticker) | "\(.id) \(.sticker.animated // false)"' "$data"
}

sticker_url() {
  echo "$DICEBEAR/$(avatar_style_for "$1")/png?seed=$(url_encode "$1")&size=$STICKER_SIZE"
}

has_no_asset_on_purpose() {
  [[ $1 == *-missing || $1 == *-missing-* ]]
}

is_undecodable_on_purpose() {
  [[ $1 == *-undecodable-* ]]
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

fetch_room_tiles() {
  local room
  while read -r room; do
    fetch "$(photo_url "$room" "$AVATAR_SIZE" "$AVATAR_SIZE")" "$assets/room-$room.png"
  done < <(room_avatars)
}

fetch_space_tiles() {
  local space
  while read -r space; do
    fetch "$(space_tile_url "$space")" "$assets/space-$space.png"
  done < <(space_avatars)
}

jpeg_frame_header_offset() {
  LC_ALL=C grep -obUaP '\xFF[\xC0\xC2]' "$1" | head -n1 | cut -d: -f1
}

declare_jpeg_size() {
  local file=$1 width=$2 height=$3 frame
  frame=$(jpeg_frame_header_offset "$file")
  [[ -n $frame ]] || return 1
  printf '%b' "$(printf '\\x%02x' $((height >> 8)) $((height & 255)) $((width >> 8)) $((width & 255)))" |
    dd of="$file" bs=1 seek=$((frame + JPEG_FRAME_HEADER_TO_HEIGHT)) conv=notrunc status=none
}

write_first_half() {
  local source=$1 destination=$2
  head -c $(($(wc -c <"$source") / 2)) "$source" >"$destination"
}

undecodable_asset() {
  local id=$1
  case ${id##*-undecodable-} in
    format) echo "$assets/thumbnail-$id.heif" ;;
    oversized) echo "$assets/thumbnail-$id.jpg" ;;
    *) echo "$assets/thumbnail-$id.png" ;;
  esac
}

write_undecodable() {
  local id=$1 width=$2 height=$3 destination=$4
  case ${id##*-undecodable-} in
    format) printf '%b' "$HEIF_BRAND_BOX" >"$destination" ;;
    oversized)
      magick -size "$UNDECODABLE_SEED_SIZE" xc:gray60 -strip "jpg:$destination" &&
        declare_jpeg_size "$destination" "$width" "$height"
      ;;
    truncated)
      magick -size "${width}x${height}" gradient: "$tmp/whole.png" &&
        write_first_half "$tmp/whole.png" "$destination"
      ;;
    *) return 1 ;;
  esac
}

make_undecodable_photo() {
  local id=$1 width=$2 height=$3 destination
  destination=$(undecodable_asset "$id")
  if already_fetched "$destination"; then
    echo "  $(basename "$destination") (kept, --force to rewrite)"
  elif write_undecodable "$id" "$width" "$height" "$destination"; then
    echo "  $(basename "$destination") (undecodable on purpose)"
  else
    echo "  $(basename "$destination") FAILED" >&2
    echo "$destination" >>"$failures"
  fi
}

fetch_photos() {
  local id width height
  while read -r id width height; do
    has_no_asset_on_purpose "$id" && continue
    if is_undecodable_on_purpose "$id"; then
      make_undecodable_photo "$id" "$width" "$height"
      continue
    fi
    fetch "$(photo_url "$id" "$width" "$height")" "$assets/thumbnail-$id.png"
  done < <(photo_messages)
}

generate_demo_videos() {
  local id width height duration
  if ! command -v ffmpeg >/dev/null; then
    echo "ffmpeg not found, skipping demo video clips" >&2
    return 0
  fi
  while read -r id width height duration; do
    has_no_asset_on_purpose "$id" && continue
    local destination="$assets/video-$id.mp4"
    already_fetched "$destination" && continue
    if ffmpeg -y -loglevel error \
      -f lavfi -i "testsrc=size=${width}x${height}:rate=25" \
      -f lavfi -i "sine=frequency=440" \
      -t "$duration" -c:v libx264 -pix_fmt yuv420p -preset veryfast \
      -c:a aac -shortest "$destination"; then
      echo "generated video-$id.mp4"
    else
      echo "video-$id" >>"$failures"
    fi
  done < <(video_clips)
}

voice_encoder_args() {
  local encoders
  encoders=$(ffmpeg -hide_banner -encoders 2>/dev/null)
  if grep -q ' libopus ' <<<"$encoders"; then
    echo "-c:a libopus -b:a 32k"
  elif grep -q ' opus ' <<<"$encoders"; then
    echo "-c:a opus -strict experimental -b:a 32k"
  else
    echo "-c:a libvorbis -q:a 2"
  fi
}

generate_demo_audio() {
  local id kind duration destination source voice_args
  if ! command -v ffmpeg >/dev/null; then
    echo "ffmpeg not found, skipping demo audio clips" >&2
    return 0
  fi
  voice_args=$(voice_encoder_args)
  while read -r id kind duration; do
    has_no_asset_on_purpose "$id" && continue
    if [[ $kind == voice ]]; then
      destination="$assets/audio-$id.ogg"
      source="0.5*sin(2*PI*210*t)*abs(sin(2*PI*0.9*t))*(0.4+0.6*abs(sin(2*PI*3.1*t)))"
    else
      destination="$assets/audio-$id.m4a"
      source="0.2*sin(2*PI*261.6*t)+0.2*sin(2*PI*329.6*t)+0.2*sin(2*PI*392*t)"
    fi
    already_fetched "$destination" && continue
    local codec_args=(-c:a aac -b:a 96k)
    [[ $kind == voice ]] && read -r -a codec_args <<<"$voice_args"
    if ffmpeg -y -loglevel error \
      -f lavfi -i "aevalsrc=$source:s=48000:d=$duration" \
      -ac 1 "${codec_args[@]}" "$destination"; then
      echo "generated $(basename "$destination")"
    else
      echo "audio-$id" >>"$failures"
    fi
  done < <(audio_clips)
}

fetch_video_posters() {
  local id width height
  while read -r id width height; do
    has_no_asset_on_purpose "$id" && continue
    fetch "$(photo_url "$id" "$width" "$height")" "$assets/thumbnail-$id.png"
  done < <(video_messages)
}

animate_sticker() {
  local source=$1 destination=$2
  magick -dispose background -delay 8 -loop 0 \
    \( "$source" -background none -rotate -6 \) \
    \( "$source" -background none -rotate 0 \) \
    \( "$source" -background none -rotate 6 \) \
    \( "$source" -background none -rotate 0 \) \
    -define webp:lossless=true "$destination"
}

fetch_sticker() {
  local id=$1 animated=$2 destination

  if [[ $animated == true ]]; then
    destination=$assets/thumbnail-$id.webp
  else
    destination=$assets/thumbnail-$id.png
  fi

  if already_fetched "$destination"; then
    echo "  $(basename "$destination") (kept, --force to refetch)"
    return
  fi

  if ! curl -sfL --max-time 20 "$(sticker_url "$id")" -o "$tmp/sticker"; then
    echo "  $(basename "$destination") FAILED" >&2
    echo "$destination" >>"$failures"
    return
  fi

  if [[ $animated == true ]]; then
    animate_sticker "$tmp/sticker" "$destination"
  else
    magick "$tmp/sticker" -background none "$destination"
  fi
  echo "  $(basename "$destination")"
}

fetch_stickers() {
  local id animated
  while read -r id animated; do
    has_no_asset_on_purpose "$id" && continue
    fetch_sticker "$id" "$animated"
  done < <(sticker_messages)
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
fetch_room_tiles
fetch_space_tiles
fetch_photos
fetch_video_posters
generate_demo_videos
generate_demo_audio
fetch_stickers
report

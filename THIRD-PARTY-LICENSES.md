# Third-Party Licenses

Rust crate dependencies are covered by their own manifests (`cargo`). This file
covers everything else: the assets embedded in the binary, and the ones used only
during development that can still show up in published screenshots.

## Twemoji (color emoji font)

The color emoji font embedded in the app (`ui/fonts/Twemoji.ttf`, family
`Twemoji`) is built from the Twemoji artwork.

- **Source:** [jdecked/twemoji](https://github.com/jdecked/twemoji), built into a
  COLRv0 font via [`u2dm/twemoji`](https://github.com/u2dm/twemoji).
- **Copyright:** Twemoji © Twitter, Inc and contributors; maintained by jdecked.
- **License:** Creative Commons Attribution 4.0 International (CC-BY 4.0) <https://creativecommons.org/licenses/by/4.0/>. Full text: [`licenses/CC-BY-4.0.txt`](licenses/CC-BY-4.0.txt).
- **Modifications:** the SVG artwork was compiled into a COLRv0 OpenType font
  with [nanoemoji](https://github.com/googlefonts/nanoemoji), and the `cmap` was subset to remove text-range codepoints (space, digits, `#`, `*`) so plain text falls back to the UI font.

## libwebrtc (calls)

Builds with the `calls` feature (on by default) link LiveKit's prebuilt WebRTC library,
downloaded by the `webrtc-sys` crate's build script.

- **Source:** [livekit/rust-sdks](https://github.com/livekit/rust-sdks) release `webrtc-89d790b`,
  `webrtc-linux-x64-release.zip` (sha256
  `b167adad5291cea0e4d66a0454d9d52d2ad714e6b0ed70f4410317d3ebde70c5`), built from Google's
  [WebRTC](https://webrtc.googlesource.com/src).
- **Copyright:** The WebRTC project authors, and the authors of the libraries it bundles
  (BoringSSL, Opus, libvpx, libyuv, abseil and others).
- **License:** BSD-3-Clause for WebRTC; each bundled library under its own license. The archive's
  `LICENSE.md` (4089 lines) lists every one of them, and a binary distribution must ship it beside
  the binary.

## Demo images

`cargo run --features demo` shows placeholder people, spaces and photos.
`scripts/gen-demo-assets.sh` fetches them into `assets/demo/`. They are gitignored
and never end up in a build, but they do show up in screenshots published with the
project (see `docs/`), which is why both sources were picked so that no attribution
is needed.

- **Avatars and space tiles:** [DiceBear](https://www.dicebear.com), using the styles
  `open-peeps`, `notionists`, `lorelei`, `pixel-art`, `shapes`, `rings`, `glass` and
  `identicon`. Their artwork is [CC0 1.0](https://creativecommons.org/publicdomain/zero/1.0/)
  and the library itself is MIT. Not every DiceBear style is CC0: `personas`, `micah` and
  `adventurer` are CC-BY 4.0, while `avataaars` and `bottts` come with their own terms. Check
  the license on `dicebear.com/styles/<name>` before adding a style to the script.
- **Photo messages:** [Lorem Picsum](https://picsum.photos), which serves photos from
  [Unsplash](https://unsplash.com) under the [Unsplash License](https://unsplash.com/license).
  No attribution is required, though the photos cannot be resold unmodified.
- **Stickers:** the CC0 DiceBear styles above. The animated one is derived locally.

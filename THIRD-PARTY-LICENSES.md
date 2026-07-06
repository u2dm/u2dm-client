# Third-Party Licenses

U2DM bundles third-party assets whose licenses require attribution. Rust crate
dependencies are covered by their own manifests (`cargo`). This file lists
non-crate assets embedded into the application binary.

## Twemoji (color emoji font)

The color emoji font embedded in the app (`ui/fonts/Twemoji.ttf`, family
`Twemoji`) is built from the Twemoji artwork.

- **Source:** [jdecked/twemoji](https://github.com/jdecked/twemoji), built into a
  COLRv0 font via [`u2dm/twemoji`](https://github.com/u2dm/twemoji).
- **Copyright:** Twemoji © Twitter, Inc and contributors; maintained by jdecked.
- **License:** Creative Commons Attribution 4.0 International (CC-BY 4.0) <https://creativecommons.org/licenses/by/4.0/>. Full text: [`licenses/CC-BY-4.0.txt`](licenses/CC-BY-4.0.txt).
- **Modifications:** the SVG artwork was compiled into a COLRv0 OpenType font
  with [nanoemoji](https://github.com/googlefonts/nanoemoji), and the `cmap` was subset to remove text-range codepoints (space, digits, `#`, `*`) so plain text falls back to the UI font.

# U2DM

Unable to Decrypt Message: a Matrix chat client built with Slint UI.

## Status

Early development. Expect incomplete features, rough edges, and breaking changes.

## Features

- Password and OAuth login
- Room list with message previews and unread and mention badges
- Spaces shown as a reorderable rail that filters the room list
- Message timeline
- Emoji picker
- Media showing and download
- SAS device verification for encrypted rooms
- Encrypted session storage using SQLite and the OS keyring

## Building

```sh
cargo run
```

The first build downloads the bundled color emoji font from the [`u2dm/twemoji`](https://github.com/u2dm/twemoji) release. It is cached at `ui/fonts/Twemoji.ttf` afterwards.

## Licenses

- Code: **AGPL-3.0-or-later**.
- Bundled third-party assets (the Twemoji emoji font): see
  [`THIRD-PARTY-LICENSES.md`](THIRD-PARTY-LICENSES.md).

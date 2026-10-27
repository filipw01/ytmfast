# Ytmfast

**YouTube Music, native and fast.** A fork of [Spotifast](https://github.com/crmne/spotifast) that keeps the same native stack (Rust, egui, fastframe) and swaps Spotify/librespot for YouTube Music.

Ytmfast is a lightweight desktop music app for Linux, macOS, and Windows. It typically aims for the same envelope as Spotifast: a small resident set, a fast cold start, and no browser engine in the UI process.

```sh
cargo run --no-default-features --features demo -- --demo
```

For a normal build (MilkDrop visualiser on by default; needs cmake, a C++ compiler, and libclang):

```sh
cargo run
```

## What changed from Spotifast

| Layer | Spotifast | Ytmfast |
|---|---|---|
| UI | egui + fastframe | same |
| Catalogue | Spotify Web API | [ytmapi-rs](https://crates.io/crates/ytmapi-rs) (Innertube) |
| Playback | librespot (Spotify Connect) | [rusty_ytdl](https://crates.io/crates/youtui-vendored-rusty_ytdl) + rodio/symphonia |
| Auth | Spotify PKCE + desktop client | YouTube Music browser cookie (and demo mode) |
| Connect / Cast / Sonos | supported | not ported yet (local playback only) |

Track URIs look like `ytmusic:track:{videoId}`.

## Features

| Feature | Status |
|---|---|
| **Library and search** | YouTube Music catalogue via Innertube; demo mode ships sample data |
| **Local playback** | Downloads AAC audio and plays through the same fastframe-audio path |
| **Queue / shuffle / repeat** | Local engine (no Connect cluster) |
| **Themes, tray, media keys** | Inherited from Spotifast / fastframe |
| **Winamp mini player / MilkDrop** | Inherited; MilkDrop needs the `milkdrop` feature (default) |
| **Spotify Connect / Cast / Sonos** | Stubbed / unavailable on YouTube Music |

## Sign in

1. Open [YouTube Music](https://music.youtube.com) in a browser and sign in.
2. Copy the `Cookie` header from an authenticated request (Network tab → any `browse` call).
3. Paste it into Ytmfast when asked, or place it where the login screen describes.

Unauthenticated use can still search and play many public tracks. Private library shelves need the cookie.

**Playback does not need Spotify Premium.** YouTube Music availability follows Google’s own limits and region rules.

## Build notes

- Rust **1.98** (see `rust-toolchain.toml`).
- Linux audio needs PipeWire or ALSA development headers (`libpipewire-0.3-dev`, `libasound2-dev`).
- `--no-default-features` skips MilkDrop and is the fastest way to iterate.
- `--features demo -- --demo` runs the UI without signing in.

## Acknowledgements

Forked from [Spotifast](https://github.com/crmne/spotifast) by Carmine Paolino (MIT). Playback and catalogue lean on [ytmapi-rs](https://github.com/nick42d/youtui) / [youtui](https://github.com/nick42d/youtui) and the vendored rusty_ytdl build they maintain. UI foundation: [egui](https://www.egui.rs) and [fastframe](https://fastframe.dev).

Ytmfast is an independent project and is not affiliated with Google, YouTube, Spotify, or Spotifast.

Licensed under the [MIT License](LICENSE).

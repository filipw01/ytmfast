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
| Auth | Spotify PKCE + desktop client | Sign in with Google (your own OAuth client), or a browser cookie (and demo mode) |
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

### Sign in with Google (recommended)

Google sign-in uses the device flow for TVs and limited-input devices, the same one
[youtui](https://github.com/nick42d/youtui) uses. Google gives no client to share, so you
create your own OAuth client once in Google Cloud. It is free.

1. In the [Google Cloud console](https://console.cloud.google.com/), create a project (or pick one)
   and enable the **YouTube Data API v3** for it. Google's guide:
   [Registering an application](https://developers.google.com/youtube/registering_an_application).
2. Configure the OAuth consent screen. While the app is in *Testing*, add your Google account as a test user.
3. Under **APIs & Services → Credentials**, choose **Create credentials → OAuth client ID** with
   application type **TVs and Limited Input devices**.
4. In Ytmfast, paste the **Client ID** and **Client secret** on the sign-in screen
   (they are also under **Settings → Account**), then click **Sign in with Google**.
5. Your browser opens Google's device page with the code filled in. Ytmfast also shows the code;
   confirm it and allow access. Ytmfast signs in as soon as Google answers.

The Client ID and secret are saved in `settings.json` (`oauth_client_id`, `oauth_client_secret`).
Google treats a limited-input client's secret as not confidential, but it is still yours: do not
share it. Instead of Settings you can set `YTMFAST_OAUTH_CLIENT_ID` and
`YTMFAST_OAUTH_CLIENT_SECRET` (youtui's `YOUTUI_OAUTH_CLIENT_ID` / `YOUTUI_OAUTH_CLIENT_SECRET`
work too); values in Settings win.

The token Google returns is kept in your system's credential store, never in `settings.json`, and
Ytmfast refreshes it on its own. Revoking access at
[myaccount.google.com/permissions](https://myaccount.google.com/permissions) signs Ytmfast out the next
time it talks to YouTube Music.

### Advanced: paste a browser cookie

Click **Advanced: paste a browser cookie** on the sign-in screen, then:

1. Open [YouTube Music](https://music.youtube.com) in a browser and sign in.
2. Copy the `Cookie` header from an authenticated request (Network tab → any `browse` call).
3. Paste it into Ytmfast and click **Sign in with the cookie**.

The cookie works like a password and is kept in the credential store too.

Unauthenticated use can still search and play many public tracks. Private library shelves need a sign-in.

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

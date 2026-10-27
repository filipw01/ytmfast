# Ytmfast

**YouTube Music, native and fast.** A Spotifast-inspired music client for the YouTube Music catalogue: home feed, search, playlists, library shelf, and a persistent player bar.

Ytmfast is a spiritual fork of [Spotifast](https://github.com/crmne/spotifast) for YouTube Music. Spotifast is a native Rust + egui Spotify client; this repo ships the same product shape as a fast web app so you can run and extend it immediately, with live catalogue browsing through [youtubei.js](https://github.com/LuanRT/YouTube.js).

## Features

| Feature | What you can do |
|---|---|
| **Home** | Quick picks and carousels from YouTube Music (or a built-in demo library). |
| **Search** | Songs, albums, playlists, and artists. |
| **Playlists & albums** | Open a collection and play the full track list. |
| **Library** | Local demo shelf of playlists, albums, and liked songs. |
| **Player** | Queue, shuffle, repeat, seek, and volume in a Spotifast-style bottom bar. |
| **Demo mode** | Browse and play without hitting YouTube Music (sample audio included). |

## Run locally

```sh
npm install
npm run dev
```

Open [http://127.0.0.1:43123](http://127.0.0.1:43123).

Production build:

```sh
npm run build
npm start
```

No API keys are required. Live mode uses YouTube Music’s public Innertube endpoints. Private library sync (liked songs / your playlists via browser cookies) is not wired yet; use **Demo mode** in the sidebar when the network path is blocked or you want offline UI.

## Playback notes

- **Demo tracks** play through local HTML audio (SoundHelix sample MP3s).
- **Live tracks** play through the YouTube IFrame Player API in the browser. Direct stream URL extraction often requires a logged-in session and is not exposed by this app.

## Stack

- Next.js 15 (App Router) + TypeScript + Tailwind
- shadcn/ui primitives
- zustand for player state
- youtubei.js for YouTube Music home, search, explore, and playlists

## Acknowledgements

Inspired by [Spotifast](https://github.com/crmne/spotifast) by Carmine Paolino (MIT). Uses [youtubei.js](https://github.com/LuanRT/YouTube.js), Lucide icons, and Manrope / Sora.

Ytmfast is an independent project and is not affiliated with Google, YouTube, Spotify, or Spotifast.

Licensed under the [MIT License](LICENSE).

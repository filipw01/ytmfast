import type {
  ArtistSummary,
  HomeFeed,
  PlaylistDetail,
  PlaylistSummary,
  SearchResults,
  Track,
} from "./types";

/** Creative Commons / sample audio for offline demo playback. */
const DEMO_AUDIO = [
  "https://www.soundhelix.com/examples/mp3/SoundHelix-Song-1.mp3",
  "https://www.soundhelix.com/examples/mp3/SoundHelix-Song-2.mp3",
  "https://www.soundhelix.com/examples/mp3/SoundHelix-Song-3.mp3",
  "https://www.soundhelix.com/examples/mp3/SoundHelix-Song-8.mp3",
  "https://www.soundhelix.com/examples/mp3/SoundHelix-Song-13.mp3",
];

const ARTISTS = [
  "Bonobo",
  "Khruangbin",
  "Nils Frahm",
  "Little Simz",
  "Floating Points",
  "Jon Hopkins",
  "Sault",
  "Four Tet",
];

const TRACK_TITLES = [
  "Rosewood",
  "Otomo",
  "Shadows",
  "Tides",
  "Elysian",
  "Closer",
  "Counterpart",
  "Sapien",
  "From You",
  "Day by Day",
  "Age of Phase",
  "Polyghost",
  "Time Moves Slow",
  "August 10",
  "So Rare",
  "Fugue",
  "Encores",
  "Sunlight",
  "My Friend the Forest",
  "Kaleidoscope",
];

const PLAYLIST_TITLES = [
  "Late night focus",
  "Sunday morning",
  "Running 2026",
  "Berlin nights",
  "Dinner party",
  "Deep work",
  "Road trip",
  "Quiet piano",
];

function cover(seed: string, size = 640): string {
  return `https://picsum.photos/seed/ytmfast-${encodeURIComponent(seed)}/${size}/${size}`;
}

function track(index: number): Track {
  const title = TRACK_TITLES[index % TRACK_TITLES.length];
  const artist = ARTISTS[index % ARTISTS.length];
  return {
    id: `demo-track-${index}`,
    title,
    artists: [{ name: artist, id: `demo-artist-${index % ARTISTS.length}` }],
    album: `${artist} Essentials`,
    thumbnail: cover(`track-${index}`, 300),
    durationSeconds: 180 + ((index * 37) % 140),
    type: "song",
  };
}

function playlist(index: number): PlaylistSummary {
  return {
    id: `demo-playlist-${index}`,
    title: PLAYLIST_TITLES[index % PLAYLIST_TITLES.length],
    thumbnail: cover(`playlist-${index}`, 300),
    author: "Ytmfast",
    trackCount: 12 + (index % 20),
    type: "playlist",
  };
}

function album(index: number): PlaylistSummary {
  const artist = ARTISTS[index % ARTISTS.length];
  return {
    id: `demo-album-${index}`,
    title: ["Fragments", "Mordechai", "All Melody", "Immunity"][index % 4],
    thumbnail: cover(`album-${index}`, 300),
    author: artist,
    year: String(2018 + (index % 7)),
    trackCount: 10 + (index % 6),
    type: "album",
  };
}

function artist(index: number): ArtistSummary {
  return {
    id: `demo-artist-${index % ARTISTS.length}`,
    name: ARTISTS[index % ARTISTS.length],
    thumbnail: cover(`artist-${index}`, 300),
    subscribers: `${(120 + index * 37) % 900}K subscribers`,
    type: "artist",
  };
}

export function demoAudioUrl(trackId: string): string | null {
  if (!trackId.startsWith("demo-track-")) return null;
  const index = Number(trackId.replace("demo-track-", "")) || 0;
  return DEMO_AUDIO[index % DEMO_AUDIO.length];
}

export function getDemoHome(): HomeFeed {
  const quickPicks = Array.from({ length: 12 }, (_, i) => track(i));
  return {
    source: "demo",
    quickPicks,
    sections: [
      {
        title: "Quick picks",
        items: quickPicks.slice(0, 8),
      },
      {
        title: "Listen again",
        items: Array.from({ length: 8 }, (_, i) => album(i)),
      },
      {
        title: "Made for you",
        items: Array.from({ length: 8 }, (_, i) => playlist(i)),
      },
      {
        title: "Your artists",
        items: Array.from({ length: 8 }, (_, i) => artist(i)),
      },
    ],
  };
}

export function getDemoSearch(query: string): SearchResults {
  const q = query.trim().toLowerCase();
  const match = (text: string) => !q || text.toLowerCase().includes(q);
  return {
    source: "demo",
    query,
    songs: Array.from({ length: 12 }, (_, i) => track(i)).filter(
      (t) =>
        match(t.title) ||
        t.artists.some((a) => match(a.name)) ||
        match(t.album ?? ""),
    ),
    albums: Array.from({ length: 6 }, (_, i) => album(i)).filter(
      (a) => match(a.title) || match(a.author ?? ""),
    ),
    playlists: Array.from({ length: 6 }, (_, i) => playlist(i)).filter((p) =>
      match(p.title),
    ),
    artists: Array.from({ length: 6 }, (_, i) => artist(i)).filter((a) =>
      match(a.name),
    ),
  };
}

export function getDemoPlaylist(id: string): PlaylistDetail | null {
  if (id.startsWith("demo-playlist-")) {
    const index = Number(id.replace("demo-playlist-", "")) || 0;
    const summary = playlist(index);
    return {
      source: "demo",
      ...summary,
      description:
        "A sample playlist for exploring Ytmfast without a YouTube Music session.",
      tracks: Array.from({ length: summary.trackCount ?? 12 }, (_, i) =>
        track(index * 3 + i),
      ),
    };
  }
  if (id.startsWith("demo-album-")) {
    const index = Number(id.replace("demo-album-", "")) || 0;
    const summary = album(index);
    return {
      source: "demo",
      ...summary,
      description: `${summary.author} · ${summary.year}`,
      tracks: Array.from({ length: summary.trackCount ?? 10 }, (_, i) =>
        track(index * 5 + i),
      ),
    };
  }
  return null;
}

export function getDemoLibrary() {
  return {
    playlists: Array.from({ length: 8 }, (_, i) => playlist(i)),
    albums: Array.from({ length: 6 }, (_, i) => album(i)),
    artists: Array.from({ length: 6 }, (_, i) => artist(i)),
    liked: Array.from({ length: 20 }, (_, i) => track(i + 2)),
  };
}

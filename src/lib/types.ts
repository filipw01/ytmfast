export type ItemType =
  | "song"
  | "video"
  | "album"
  | "playlist"
  | "artist"
  | "podcast"
  | "episode";

export interface ArtistRef {
  id?: string;
  name: string;
}

export interface Track {
  id: string;
  title: string;
  artists: ArtistRef[];
  album?: string;
  albumId?: string;
  thumbnail?: string;
  durationSeconds?: number;
  durationText?: string;
  explicit?: boolean;
  type: "song" | "video";
}

export interface PlaylistSummary {
  id: string;
  title: string;
  thumbnail?: string;
  author?: string;
  trackCount?: number;
  description?: string;
  type: "playlist" | "album";
  year?: string;
}

export interface ArtistSummary {
  id: string;
  name: string;
  thumbnail?: string;
  subscribers?: string;
  type: "artist";
}

export interface HomeSection {
  title: string;
  items: Array<Track | PlaylistSummary | ArtistSummary>;
}

export interface HomeFeed {
  source: "live" | "demo";
  sections: HomeSection[];
  quickPicks: Track[];
}

export interface SearchResults {
  source: "live" | "demo";
  query: string;
  songs: Track[];
  albums: PlaylistSummary[];
  playlists: PlaylistSummary[];
  artists: ArtistSummary[];
}

export interface PlaylistDetail {
  source: "live" | "demo";
  id: string;
  title: string;
  description?: string;
  thumbnail?: string;
  author?: string;
  year?: string;
  trackCount?: number;
  type: "playlist" | "album";
  tracks: Track[];
}

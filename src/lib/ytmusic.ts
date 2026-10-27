import { Innertube } from "youtubei.js";
import type {
  ArtistSummary,
  HomeFeed,
  HomeSection,
  PlaylistDetail,
  PlaylistSummary,
  SearchResults,
  Track,
} from "./types";

let clientPromise: Promise<Innertube> | null = null;

export function getInnertube(): Promise<Innertube> {
  if (!clientPromise) {
    clientPromise = Innertube.create().catch((error) => {
      clientPromise = null;
      throw error;
    });
  }
  return clientPromise;
}

function thumbFrom(item: {
  thumbnail?: { contents?: Array<{ url?: string }>; url?: string };
  thumbnails?: Array<{ url?: string }>;
}): string | undefined {
  return (
    item.thumbnail?.contents?.[0]?.url ||
    item.thumbnails?.[0]?.url ||
    (typeof item.thumbnail?.url === "string" ? item.thumbnail.url : undefined)
  );
}

function artistsFrom(item: {
  artists?: Array<{ name?: string; channel_id?: string }>;
  authors?: Array<{ name?: string; channel_id?: string }>;
  author?: { name?: string; channel_id?: string } | string;
}): Track["artists"] {
  const fromList = (list?: Array<{ name?: string; channel_id?: string }>) =>
    (list || [])
      .filter((a) => a.name)
      .map((a) => ({ name: a.name!, id: a.channel_id }));

  const artists = fromList(item.artists);
  if (artists.length) return artists;

  const authors = fromList(item.authors);
  if (authors.length) return authors;

  if (typeof item.author === "string" && item.author) {
    return [{ name: item.author }];
  }
  if (item.author && typeof item.author === "object" && item.author.name) {
    return [{ name: item.author.name, id: item.author.channel_id }];
  }
  return [];
}

function parseDurationText(text?: string): number | undefined {
  if (!text) return undefined;
  const parts = text.split(":").map((p) => Number(p));
  if (parts.some((n) => Number.isNaN(n))) return undefined;
  if (parts.length === 3) return parts[0] * 3600 + parts[1] * 60 + parts[2];
  if (parts.length === 2) return parts[0] * 60 + parts[1];
  return parts[0];
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function asTrack(item: any): Track | null {
  if (!item?.id) return null;
  const itemType = item.item_type as string | undefined;
  if (itemType && itemType !== "song" && itemType !== "video") return null;
  const title =
    typeof item.title === "string"
      ? item.title
      : item.title?.toString?.() || item.name?.toString?.();
  if (!title) return null;
  const durationText =
    item.duration?.text ||
    (typeof item.duration === "string" ? item.duration : undefined);
  return {
    id: String(item.id),
    title: String(title),
    artists: artistsFrom(item),
    album:
      typeof item.album === "string"
        ? item.album
        : item.album?.name || item.album?.toString?.(),
    albumId: item.album?.id,
    thumbnail: thumbFrom(item),
    durationText,
    durationSeconds: parseDurationText(durationText),
    type: itemType === "video" ? "video" : "song",
  };
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function asPlaylist(item: any): PlaylistSummary | null {
  if (!item?.id) return null;
  const itemType = item.item_type as string | undefined;
  const id = String(item.id);
  const looksAlbum =
    itemType === "album" || id.startsWith("MPREb_") || id.startsWith("OLAK5");
  const looksPlaylist =
    itemType === "playlist" ||
    id.startsWith("VL") ||
    id.startsWith("PL") ||
    id.startsWith("RD");
  if (!looksAlbum && !looksPlaylist && itemType && itemType !== "album") {
    return null;
  }
  const title =
    typeof item.title === "string"
      ? item.title
      : item.title?.toString?.() || item.name?.toString?.();
  if (!title) return null;
  return {
    id,
    title: String(title),
    thumbnail: thumbFrom(item),
    author:
      artistsFrom(item)[0]?.name ||
      (typeof item.author === "string" ? item.author : item.author?.name),
    year: item.year ? String(item.year) : undefined,
    trackCount:
      typeof item.item_count === "number"
        ? item.item_count
        : typeof item.song_count === "number"
          ? item.song_count
          : undefined,
    type: looksAlbum ? "album" : "playlist",
  };
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function asArtist(item: any): ArtistSummary | null {
  if (!item?.id) return null;
  if (item.item_type && item.item_type !== "artist") return null;
  const name =
    typeof item.name === "string"
      ? item.name
      : item.name?.toString?.() ||
        (typeof item.title === "string"
          ? item.title
          : item.title?.toString?.());
  if (!name) return null;
  return {
    id: String(item.id),
    name: String(name),
    thumbnail: thumbFrom(item),
    subscribers:
      typeof item.subscribers === "string"
        ? item.subscribers
        : item.subscribers?.toString?.(),
    type: "artist",
  };
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function normalizeItem(item: any) {
  if (!item) return null;
  if (item.item_type === "artist" || String(item.id || "").startsWith("UC")) {
    return asArtist(item);
  }
  if (
    item.item_type === "album" ||
    item.item_type === "playlist" ||
    String(item.id || "").startsWith("VL") ||
    String(item.id || "").startsWith("PL") ||
    String(item.id || "").startsWith("MPREb_") ||
    String(item.id || "").startsWith("OLAK5")
  ) {
    return asPlaylist(item);
  }
  return asTrack(item);
}

export async function fetchHomeFeed(): Promise<HomeFeed> {
  const yt = await getInnertube();
  const home = await yt.music.getHomeFeed();
  const sections: HomeSection[] = [];
  let quickPicks: Track[] = [];

  for (const section of home.sections ?? []) {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const raw = section as any;
    const title =
      raw.header?.title?.toString?.() ||
      raw.title?.toString?.() ||
      "Recommended";
    const contents = raw.contents ?? raw.items ?? [];
    const items = contents
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      .map((item: any) => normalizeItem(item))
      .filter(Boolean) as HomeSection["items"];

    if (!items.length) continue;
    sections.push({ title: String(title), items });

    if (/quick picks/i.test(String(title))) {
      quickPicks = items.filter((i): i is Track => "artists" in i && "type" in i && (i.type === "song" || i.type === "video"));
    }
  }

  if (!quickPicks.length) {
    quickPicks = sections
      .flatMap((s) => s.items)
      .filter((i): i is Track => "artists" in i && (i as Track).type !== undefined)
      .slice(0, 12);
  }

  return { source: "live", sections, quickPicks };
}

export async function fetchSearch(query: string): Promise<SearchResults> {
  const yt = await getInnertube();
  const search = await yt.music.search(query);
  const songs: Track[] = [];
  const albums: PlaylistSummary[] = [];
  const playlists: PlaylistSummary[] = [];
  const artists: ArtistSummary[] = [];

  for (const section of search.contents ?? []) {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const raw = section as any;
    const contents = raw.contents ?? raw.items ?? (raw.item_type ? [raw] : []);
    for (const item of contents) {
      const track = asTrack(item);
      if (track) {
        songs.push(track);
        continue;
      }
      const artist = asArtist(item);
      if (artist) {
        artists.push(artist);
        continue;
      }
      const collection = asPlaylist(item);
      if (collection) {
        if (collection.type === "album") albums.push(collection);
        else playlists.push(collection);
      }
    }
  }

  return {
    source: "live",
    query,
    songs: uniqueById(songs),
    albums: uniqueById(albums),
    playlists: uniqueById(playlists),
    artists: uniqueById(artists),
  };
}

function uniqueById<T extends { id: string }>(items: T[]): T[] {
  const seen = new Set<string>();
  return items.filter((item) => {
    if (seen.has(item.id)) return false;
    seen.add(item.id);
    return true;
  });
}

export async function fetchPlaylist(id: string): Promise<PlaylistDetail> {
  const yt = await getInnertube();
  const playlist = await yt.music.getPlaylist(id);
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const header = playlist.header as any;
  const title =
    header?.title?.toString?.() ||
    header?.name?.toString?.() ||
    "Playlist";
  const contents =
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    ((playlist as any).items ?? (playlist as any).contents ?? []) as any[];

  const tracks = contents
    .map((item) => asTrack(item))
    .filter(Boolean) as Track[];

  const looksAlbum =
    id.startsWith("MPREb_") || id.startsWith("OLAK5") || /album/i.test(title);

  return {
    source: "live",
    id,
    title: String(title),
    description: header?.description?.toString?.() || undefined,
    thumbnail:
      thumbFrom(header) ||
      header?.thumbnail?.contents?.[0]?.url ||
      tracks[0]?.thumbnail,
    author:
      header?.author?.name ||
      header?.subtitle?.toString?.() ||
      artistsFrom(header)[0]?.name,
    year: header?.year ? String(header.year) : undefined,
    trackCount: tracks.length,
    type: looksAlbum ? "album" : "playlist",
    tracks,
  };
}

export async function fetchExploreCharts(): Promise<HomeSection[]> {
  const yt = await getInnertube();
  const explore = await yt.music.getExplore();
  const sections: HomeSection[] = [];
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const rawSections = ((explore as any).sections ?? []) as any[];
  for (const section of rawSections) {
    const title =
      section.header?.title?.toString?.() ||
      section.title?.toString?.() ||
      "Explore";
    const contents = section.contents ?? section.items ?? [];
    const items = contents
      .map((item: unknown) => normalizeItem(item))
      .filter(Boolean) as HomeSection["items"];
    if (items.length) sections.push({ title: String(title), items });
  }
  return sections;
}

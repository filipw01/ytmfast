"use client";

import { useEffect, useState, useTransition } from "react";
import { Search } from "lucide-react";
import { Input } from "@/components/ui/input";
import { MediaTile } from "@/components/media-tile";
import { TrackRow } from "@/components/track-row";
import { EmptyState, LoadingSkeleton } from "@/components/views/home-view";
import type { PlaylistSummary, SearchResults } from "@/lib/types";
import { usePlayerStore } from "@/lib/player-store";

export function SearchView() {
  const demoMode = usePlayerStore((s) => s.demoMode);
  const query = usePlayerStore((s) => s.searchQuery);
  const setSearchQuery = usePlayerStore((s) => s.setSearchQuery);
  const playTrack = usePlayerStore((s) => s.playTrack);
  const setView = usePlayerStore((s) => s.setView);
  const current = usePlayerStore((s) => s.current());
  const isPlaying = usePlayerStore((s) => s.isPlaying);
  const [results, setResults] = useState<SearchResults | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pending, startTransition] = useTransition();

  useEffect(() => {
    if (!query.trim()) {
      setResults(null);
      setError(null);
      return;
    }
    const handle = window.setTimeout(() => {
      startTransition(async () => {
        try {
          const res = await fetch(
            `/api/search?q=${encodeURIComponent(query)}${demoMode ? "&demo=1" : ""}`,
          );
          const data = await res.json();
          setResults(data);
          setError(data.error || null);
        } catch (err) {
          setError(err instanceof Error ? err.message : "Search failed");
        }
      });
    }, 280);
    return () => window.clearTimeout(handle);
  }, [query, demoMode]);

  return (
    <div className="space-y-6 pb-8">
      <div>
        <h1 className="font-display text-3xl font-semibold tracking-tight text-[var(--yt-text)]">
          Search
        </h1>
        <p className="mt-2 text-sm text-[var(--yt-secondary)]">
          Songs, albums, playlists, and artists from YouTube Music.
        </p>
      </div>

      <div className="relative max-w-xl">
        <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-[var(--yt-dim)]" />
        <Input
          value={query}
          onChange={(event) => setSearchQuery(event.target.value)}
          placeholder="What do you want to listen to?"
          className="h-12 border-[var(--yt-outline)] bg-[var(--yt-surface)] pl-10 text-[var(--yt-text)] placeholder:text-[var(--yt-dim)]"
          autoFocus
        />
      </div>

      {!query.trim() ? (
        <EmptyState
          title="Find your next track"
          detail="Try an artist, album, or song. Live mode hits YouTube Music; demo mode filters the sample library."
        />
      ) : pending && !results ? (
        <LoadingSkeleton label="Searching…" />
      ) : results ? (
        <div className="space-y-8">
          {error ? (
            <p className="text-xs text-[var(--yt-warning)]">{error}</p>
          ) : null}

          {results.songs.length ? (
            <section className="space-y-2">
              <h2 className="font-display text-xl font-semibold">Songs</h2>
              <div className="rounded-lg bg-[var(--yt-panel)]/40 p-1">
                {results.songs.slice(0, 12).map((track, index) => (
                  <TrackRow
                    key={track.id}
                    track={track}
                    index={index}
                    active={current?.id === track.id}
                    playing={current?.id === track.id && isPlaying}
                    onPlay={() => playTrack(track, results.songs)}
                  />
                ))}
              </div>
            </section>
          ) : null}

          {results.albums.length ? (
            <Shelf
              title="Albums"
              items={results.albums}
              onOpen={(id, title) => setView({ kind: "playlist", id, title })}
            />
          ) : null}
          {results.playlists.length ? (
            <Shelf
              title="Playlists"
              items={results.playlists}
              onOpen={(id, title) => setView({ kind: "playlist", id, title })}
            />
          ) : null}
          {results.artists.length ? (
            <section className="space-y-4">
              <h2 className="font-display text-xl font-semibold">Artists</h2>
              <div className="flex gap-4 overflow-x-auto pb-2">
                {results.artists.map((artist) => (
                  <MediaTile
                    key={artist.id}
                    item={artist}
                    onOpen={() => setSearchQuery(artist.name)}
                  />
                ))}
              </div>
            </section>
          ) : null}

          {!results.songs.length &&
          !results.albums.length &&
          !results.playlists.length &&
          !results.artists.length ? (
            <EmptyState
              title="No matches"
              detail={`Nothing turned up for “${query}”.`}
            />
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

function Shelf({
  title,
  items,
  onOpen,
}: {
  title: string;
  items: PlaylistSummary[];
  onOpen: (id: string, title: string) => void;
}) {
  return (
    <section className="space-y-4">
      <h2 className="font-display text-xl font-semibold">{title}</h2>
      <div className="flex gap-4 overflow-x-auto pb-2">
        {items.map((item) => (
          <MediaTile
            key={item.id}
            item={item}
            onOpen={() => onOpen(item.id, item.title)}
          />
        ))}
      </div>
    </section>
  );
}

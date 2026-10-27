"use client";

import { useMemo } from "react";
import { MediaTile } from "@/components/media-tile";
import { TrackRow } from "@/components/track-row";
import { getDemoLibrary } from "@/lib/demo-data";
import { usePlayerStore } from "@/lib/player-store";

export function LibraryView() {
  const playTrack = usePlayerStore((s) => s.playTrack);
  const setView = usePlayerStore((s) => s.setView);
  const current = usePlayerStore((s) => s.current());
  const isPlaying = usePlayerStore((s) => s.isPlaying);
  const library = useMemo(() => getDemoLibrary(), []);

  return (
    <div className="space-y-8 pb-8">
      <div>
        <h1 className="font-display text-3xl font-semibold tracking-tight text-[var(--yt-text)]">
          Your library
        </h1>
        <p className="mt-2 max-w-xl text-sm text-[var(--yt-secondary)]">
          Local library shelf for the Ytmfast slice. Sign-in for your private
          YouTube Music library can land later; demo content keeps the UI usable
          today.
        </p>
      </div>

      <section className="space-y-4">
        <h2 className="font-display text-xl font-semibold">Playlists</h2>
        <div className="flex gap-4 overflow-x-auto pb-2">
          {library.playlists.map((playlist) => (
            <MediaTile
              key={playlist.id}
              item={playlist}
              onOpen={() =>
                setView({
                  kind: "playlist",
                  id: playlist.id,
                  title: playlist.title,
                })
              }
            />
          ))}
        </div>
      </section>

      <section className="space-y-4">
        <h2 className="font-display text-xl font-semibold">Albums</h2>
        <div className="flex gap-4 overflow-x-auto pb-2">
          {library.albums.map((album) => (
            <MediaTile
              key={album.id}
              item={album}
              onOpen={() =>
                setView({ kind: "playlist", id: album.id, title: album.title })
              }
            />
          ))}
        </div>
      </section>

      <section className="space-y-2">
        <h2 className="font-display text-xl font-semibold">Liked songs</h2>
        <div className="rounded-lg bg-[var(--yt-panel)]/40 p-1">
          {library.liked.map((track, index) => (
            <TrackRow
              key={track.id}
              track={track}
              index={index}
              active={current?.id === track.id}
              playing={current?.id === track.id && isPlaying}
              onPlay={() => playTrack(track, library.liked)}
            />
          ))}
        </div>
      </section>
    </div>
  );
}

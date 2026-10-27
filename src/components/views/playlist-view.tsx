"use client";

import Image from "next/image";
import { useEffect, useState } from "react";
import { Play } from "lucide-react";
import { Button } from "@/components/ui/button";
import { TrackRow } from "@/components/track-row";
import { EmptyState, LoadingSkeleton } from "@/components/views/home-view";
import type { PlaylistDetail } from "@/lib/types";
import { usePlayerStore } from "@/lib/player-store";

export function PlaylistView({
  id,
  titleHint,
}: {
  id: string;
  titleHint?: string;
}) {
  const demoMode = usePlayerStore((s) => s.demoMode);
  const playQueue = usePlayerStore((s) => s.playQueue);
  const playTrack = usePlayerStore((s) => s.playTrack);
  const current = usePlayerStore((s) => s.current());
  const isPlaying = usePlayerStore((s) => s.isPlaying);
  const [detail, setDetail] = useState<PlaylistDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    fetch(`/api/playlist/${encodeURIComponent(id)}${demoMode ? "?demo=1" : ""}`)
      .then(async (res) => {
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || "Failed to load playlist");
        if (!cancelled) setDetail(data);
      })
      .catch((err: Error) => {
        if (!cancelled) setError(err.message);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [id, demoMode]);

  if (loading) {
    return <LoadingSkeleton label={`Loading ${titleHint || "playlist"}…`} />;
  }

  if (!detail) {
    return (
      <EmptyState
        title="Could not open this collection"
        detail={error || "Playlist unavailable."}
      />
    );
  }

  return (
    <div className="space-y-6 pb-8">
      <div className="flex flex-col gap-6 sm:flex-row sm:items-end">
        <div className="relative h-48 w-48 shrink-0 overflow-hidden rounded-md bg-[var(--yt-surface)] shadow-[0_24px_80px_rgba(0,0,0,0.45)]">
          {detail.thumbnail ? (
            <Image
              src={detail.thumbnail}
              alt=""
              fill
              sizes="192px"
              className="object-cover"
              unoptimized
            />
          ) : null}
        </div>
        <div className="min-w-0 flex-1">
          <div className="text-xs font-semibold uppercase tracking-[0.16em] text-[var(--yt-secondary)]">
            {detail.type}
          </div>
          <h1 className="mt-2 font-display text-4xl font-semibold tracking-tight text-[var(--yt-text)]">
            {detail.title}
          </h1>
          <p className="mt-3 text-sm text-[var(--yt-secondary)]">
            {[detail.author, detail.year, `${detail.tracks.length} songs`]
              .filter(Boolean)
              .join(" · ")}
          </p>
          {detail.description ? (
            <p className="mt-2 line-clamp-2 max-w-2xl text-sm text-[var(--yt-dim)]">
              {detail.description}
            </p>
          ) : null}
          <Button
            className="mt-5 h-11 rounded-full bg-[var(--yt-accent)] px-6 text-[var(--yt-on-accent)] hover:bg-[var(--yt-accent-hover)]"
            onClick={() => playQueue(detail.tracks, 0)}
            disabled={!detail.tracks.length}
          >
            <Play className="h-4 w-4 fill-current" />
            Play
          </Button>
        </div>
      </div>

      <div className="rounded-lg bg-[var(--yt-panel)]/40 p-1">
        {detail.tracks.map((track, index) => (
          <TrackRow
            key={`${track.id}-${index}`}
            track={track}
            index={index}
            active={current?.id === track.id}
            playing={current?.id === track.id && isPlaying}
            onPlay={() => playTrack(track, detail.tracks)}
          />
        ))}
      </div>
    </div>
  );
}

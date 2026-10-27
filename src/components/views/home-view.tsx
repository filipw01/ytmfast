"use client";

import { useEffect, useState } from "react";
import { MediaTile, QuickAccessTile } from "@/components/media-tile";
import { greeting } from "@/lib/utils";
import type { HomeFeed, PlaylistSummary, Track } from "@/lib/types";
import { usePlayerStore } from "@/lib/player-store";

export function HomeView() {
  const demoMode = usePlayerStore((s) => s.demoMode);
  const playTrack = usePlayerStore((s) => s.playTrack);
  const setView = usePlayerStore((s) => s.setView);
  const [feed, setFeed] = useState<HomeFeed | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    fetch(`/api/home${demoMode ? "?demo=1" : ""}`)
      .then(async (res) => {
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || "Failed to load home");
        if (!cancelled) {
          setFeed(data);
          if (data.error) setError(data.error);
        }
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
  }, [demoMode]);

  const openItem = (item: HomeFeed["sections"][number]["items"][number]) => {
    if (item.type === "song" || item.type === "video") {
      const tracks =
        feed?.quickPicks?.length
          ? feed.quickPicks
          : feed?.sections
              .flatMap((s) => s.items)
              .filter((i): i is Track => i.type === "song" || i.type === "video") || [
              item,
            ];
      playTrack(item, tracks);
      return;
    }
    if (item.type === "playlist" || item.type === "album") {
      setView({ kind: "playlist", id: item.id, title: item.title });
    }
  };

  const playCollection = async (item: PlaylistSummary) => {
    const res = await fetch(
      `/api/playlist/${encodeURIComponent(item.id)}${demoMode ? "?demo=1" : ""}`,
    );
    const data = await res.json();
    if (data.tracks?.length) {
      playTrack(data.tracks[0], data.tracks);
    }
  };

  if (loading) {
    return <LoadingSkeleton label="Loading your home mix…" />;
  }

  if (!feed) {
    return (
      <EmptyState
        title="Home unavailable"
        detail={error || "Could not reach YouTube Music."}
      />
    );
  }

  const quickAccess = [
    ...feed.quickPicks.slice(0, 4),
    ...feed.sections
      .flatMap((s) => s.items)
      .filter((i): i is PlaylistSummary => i.type === "playlist" || i.type === "album")
      .slice(0, 4),
  ].slice(0, 8);

  return (
    <div className="space-y-8 pb-8">
      <div>
        <h1 className="font-display text-3xl font-semibold tracking-tight text-[var(--yt-text)] animate-fade-up">
          {greeting()}
        </h1>
        <p className="mt-2 max-w-xl text-sm text-[var(--yt-secondary)] animate-fade-up delay-75">
          {feed.source === "demo"
            ? "Demo library inspired by Spotifast. Flip off demo mode in the sidebar for live YouTube Music."
            : "Browsing YouTube Music. Double-click any track to play in the bottom bar."}
        </p>
        {error ? (
          <p className="mt-2 text-xs text-[var(--yt-warning)]">{error}</p>
        ) : null}
      </div>

      <div className="grid grid-cols-1 gap-2 sm:grid-cols-2 xl:grid-cols-4 animate-fade-up delay-100">
        {quickAccess.map((item) => (
          <QuickAccessTile
            key={`${item.type}-${item.id}`}
            item={item as Track | PlaylistSummary}
            onOpen={() => openItem(item)}
            onPlay={() => {
              if (item.type === "song" || item.type === "video") {
                playTrack(item, feed.quickPicks);
              } else if (item.type === "playlist" || item.type === "album") {
                void playCollection(item);
              }
            }}
          />
        ))}
      </div>

      {feed.sections.map((section) => (
        <section key={section.title} className="space-y-4">
          <h2 className="font-display text-xl font-semibold text-[var(--yt-text)]">
            {section.title}
          </h2>
          <div className="flex gap-4 overflow-x-auto pb-2 scrollbar-thin">
            {section.items.map((item) => (
              <MediaTile
                key={`${section.title}-${item.type}-${item.id}`}
                item={item}
                onOpen={() => openItem(item)}
                onPlay={
                  item.type === "song" || item.type === "video"
                    ? () =>
                        playTrack(
                          item,
                          section.items.filter(
                            (i): i is Track =>
                              i.type === "song" || i.type === "video",
                          ),
                        )
                    : item.type === "playlist" || item.type === "album"
                      ? () => void playCollection(item)
                      : undefined
                }
              />
            ))}
          </div>
        </section>
      ))}
    </div>
  );
}

export function LoadingSkeleton({ label }: { label: string }) {
  return (
    <div className="space-y-6">
      <div className="h-9 w-56 animate-pulse rounded bg-[var(--yt-surface)]" />
      <p className="text-sm text-[var(--yt-secondary)]">{label}</p>
      <div className="grid grid-cols-2 gap-3 xl:grid-cols-4">
        {Array.from({ length: 8 }).map((_, i) => (
          <div
            key={i}
            className="h-16 animate-pulse rounded-md bg-[var(--yt-surface)]"
          />
        ))}
      </div>
    </div>
  );
}

export function EmptyState({
  title,
  detail,
}: {
  title: string;
  detail: string;
}) {
  return (
    <div className="flex min-h-[40vh] flex-col items-center justify-center text-center">
      <h2 className="font-display text-2xl font-semibold text-[var(--yt-text)]">
        {title}
      </h2>
      <p className="mt-2 max-w-md text-sm text-[var(--yt-secondary)]">{detail}</p>
    </div>
  );
}

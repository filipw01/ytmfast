"use client";

import { useEffect, useState } from "react";
import { MediaTile } from "@/components/media-tile";
import { EmptyState, LoadingSkeleton } from "@/components/views/home-view";
import type { HomeSection, PlaylistSummary, Track } from "@/lib/types";
import { usePlayerStore } from "@/lib/player-store";

export function ExploreView() {
  const demoMode = usePlayerStore((s) => s.demoMode);
  const playTrack = usePlayerStore((s) => s.playTrack);
  const setView = usePlayerStore((s) => s.setView);
  const [sections, setSections] = useState<HomeSection[]>([]);
  const [source, setSource] = useState<"live" | "demo">("live");
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    fetch(`/api/explore${demoMode ? "?demo=1" : ""}`)
      .then(async (res) => {
        const data = await res.json();
        if (!cancelled) {
          setSections(data.sections || []);
          setSource(data.source || "demo");
          setError(data.error || null);
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

  if (loading) return <LoadingSkeleton label="Loading explore…" />;
  if (!sections.length) {
    return (
      <EmptyState
        title="Nothing to explore"
        detail={error || "Charts and moods did not load."}
      />
    );
  }

  return (
    <div className="space-y-8 pb-8">
      <div>
        <h1 className="font-display text-3xl font-semibold tracking-tight text-[var(--yt-text)]">
          Explore
        </h1>
        <p className="mt-2 text-sm text-[var(--yt-secondary)]">
          {source === "demo"
            ? "Demo charts and moods."
            : "New releases, moods, and charts from YouTube Music."}
        </p>
        {error ? (
          <p className="mt-2 text-xs text-[var(--yt-warning)]">{error}</p>
        ) : null}
      </div>

      {sections.map((section) => (
        <section key={section.title} className="space-y-4">
          <h2 className="font-display text-xl font-semibold">{section.title}</h2>
          <div className="flex gap-4 overflow-x-auto pb-2">
            {section.items.map((item) => (
              <MediaTile
                key={`${section.title}-${item.id}`}
                item={item}
                onOpen={() => {
                  if (item.type === "song" || item.type === "video") {
                    playTrack(
                      item,
                      section.items.filter(
                        (i): i is Track =>
                          i.type === "song" || i.type === "video",
                      ),
                    );
                  } else if (item.type === "playlist" || item.type === "album") {
                    setView({
                      kind: "playlist",
                      id: item.id,
                      title: (item as PlaylistSummary).title,
                    });
                  }
                }}
              />
            ))}
          </div>
        </section>
      ))}
    </div>
  );
}

"use client";

import { useMemo } from "react";
import { Sidebar } from "@/components/layout/sidebar";
import { PlayerBar } from "@/components/player/player-bar";
import { PlaybackEngine } from "@/components/player/youtube-engine";
import { ExploreView } from "@/components/views/explore-view";
import { HomeView } from "@/components/views/home-view";
import { LibraryView } from "@/components/views/library-view";
import { PlaylistView } from "@/components/views/playlist-view";
import { SearchView } from "@/components/views/search-view";
import { getDemoLibrary } from "@/lib/demo-data";
import { usePlayerStore } from "@/lib/player-store";

export function AppShell() {
  const view = usePlayerStore((s) => s.view);
  const playlists = useMemo(() => getDemoLibrary().playlists, []);

  return (
    <div className="flex h-dvh flex-col overflow-hidden bg-[var(--yt-window)] text-[var(--yt-text)]">
      <div className="flex min-h-0 flex-1">
        <div className="hidden md:block">
          <Sidebar playlists={playlists} />
        </div>
        <main className="relative min-w-0 flex-1 overflow-y-auto">
          <div className="pointer-events-none absolute inset-x-0 top-0 h-72 bg-[radial-gradient(ellipse_at_top_left,rgba(255,0,51,0.18),transparent_55%),radial-gradient(ellipse_at_top_right,rgba(40,40,48,0.55),transparent_45%)]" />
          <div className="relative px-4 py-6 sm:px-8">
            <MobileNav />
            {view === "home" ? <HomeView /> : null}
            {view === "search" ? <SearchView /> : null}
            {view === "library" ? <LibraryView /> : null}
            {view === "explore" ? <ExploreView /> : null}
            {typeof view === "object" && view.kind === "playlist" ? (
              <PlaylistView id={view.id} titleHint={view.title} />
            ) : null}
          </div>
        </main>
      </div>
      <PlayerBar />
      <PlaybackEngine />
    </div>
  );
}

function MobileNav() {
  const view = usePlayerStore((s) => s.view);
  const setView = usePlayerStore((s) => s.setView);
  const items = [
    { id: "home" as const, label: "Home" },
    { id: "search" as const, label: "Search" },
    { id: "explore" as const, label: "Explore" },
    { id: "library" as const, label: "Library" },
  ];

  return (
    <div className="mb-5 flex gap-2 overflow-x-auto md:hidden">
      {items.map((item) => (
        <button
          key={item.id}
          type="button"
          onClick={() => setView(item.id)}
          className={`rounded-full px-3 py-1.5 text-sm transition ${
            view === item.id
              ? "bg-[var(--yt-text)] text-[var(--yt-window)]"
              : "bg-[var(--yt-surface)] text-[var(--yt-secondary)]"
          }`}
        >
          {item.label}
        </button>
      ))}
    </div>
  );
}

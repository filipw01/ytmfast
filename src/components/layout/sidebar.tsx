"use client";

import {
  Compass,
  Home,
  Library,
  ListMusic,
  Search,
  Sparkles,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { usePlayerStore, type ViewId } from "@/lib/player-store";
import type { PlaylistSummary } from "@/lib/types";

const NAV: Array<{ id: ViewId; label: string; icon: typeof Home }> = [
  { id: "home", label: "Home", icon: Home },
  { id: "search", label: "Search", icon: Search },
  { id: "explore", label: "Explore", icon: Compass },
  { id: "library", label: "Library", icon: Library },
];

export function Sidebar({ playlists }: { playlists: PlaylistSummary[] }) {
  const view = usePlayerStore((s) => s.view);
  const setView = usePlayerStore((s) => s.setView);
  const demoMode = usePlayerStore((s) => s.demoMode);
  const setDemoMode = usePlayerStore((s) => s.setDemoMode);

  const activeId =
    typeof view === "string" ? view : view.kind === "playlist" ? view.id : "";

  return (
    <aside className="flex h-full w-[248px] shrink-0 flex-col border-r border-[var(--yt-outline)] bg-[var(--yt-panel)]">
      <div className="px-5 pb-2 pt-5">
        <div className="flex items-center gap-2">
          <div className="flex h-8 w-8 items-center justify-center rounded-md bg-[var(--yt-accent)] text-[var(--yt-on-accent)]">
            <Sparkles className="h-4 w-4" />
          </div>
          <div>
            <div className="font-display text-lg font-semibold tracking-tight text-[var(--yt-text)]">
              Ytmfast
            </div>
            <div className="text-[11px] text-[var(--yt-dim)]">
              YouTube Music, fast
            </div>
          </div>
        </div>
      </div>

      <nav className="mt-4 space-y-1 px-3">
        {NAV.map((item) => {
          const Icon = item.icon;
          const active = view === item.id;
          return (
            <button
              key={String(item.id)}
              type="button"
              onClick={() => setView(item.id)}
              className={cn(
                "flex w-full items-center gap-3 rounded-md px-3 py-2.5 text-sm font-medium transition",
                active
                  ? "bg-[var(--yt-surface)] text-[var(--yt-text)]"
                  : "text-[var(--yt-secondary)] hover:bg-[var(--yt-surface)] hover:text-[var(--yt-text)]",
              )}
            >
              <Icon className="h-4 w-4" />
              {item.label}
            </button>
          );
        })}
      </nav>

      <div className="mx-5 my-4 h-px bg-[var(--yt-outline)]" />

      <div className="px-5 text-[11px] font-semibold uppercase tracking-[0.14em] text-[var(--yt-dim)]">
        Playlists
      </div>
      <div className="mt-2 flex-1 space-y-0.5 overflow-y-auto px-3 pb-4">
        {playlists.map((playlist) => {
          const active = activeId === playlist.id;
          return (
            <button
              key={playlist.id}
              type="button"
              onClick={() =>
                setView({
                  kind: "playlist",
                  id: playlist.id,
                  title: playlist.title,
                })
              }
              className={cn(
                "flex w-full items-center gap-2 rounded-md px-3 py-2 text-left text-sm transition",
                active
                  ? "bg-[var(--yt-surface)] text-[var(--yt-text)]"
                  : "text-[var(--yt-secondary)] hover:bg-[var(--yt-surface)] hover:text-[var(--yt-text)]",
              )}
            >
              <ListMusic className="h-3.5 w-3.5 shrink-0 opacity-70" />
              <span className="truncate">{playlist.title}</span>
            </button>
          );
        })}
      </div>

      <div className="border-t border-[var(--yt-outline)] p-4">
        <button
          type="button"
          onClick={() => setDemoMode(!demoMode)}
          className={cn(
            "w-full rounded-md px-3 py-2 text-left text-xs transition",
            demoMode
              ? "bg-[var(--yt-accent)]/15 text-[var(--yt-accent)]"
              : "bg-[var(--yt-surface)] text-[var(--yt-secondary)] hover:text-[var(--yt-text)]",
          )}
        >
          <div className="font-semibold">
            {demoMode ? "Demo mode on" : "Use live YouTube Music"}
          </div>
          <div className="mt-0.5 opacity-80">
            {demoMode
              ? "Sample library with local audio"
              : "Browse the public catalogue"}
          </div>
        </button>
      </div>
    </aside>
  );
}

"use client";

import Image from "next/image";
import { Play } from "lucide-react";
import { cn } from "@/lib/utils";
import type { ArtistSummary, PlaylistSummary, Track } from "@/lib/types";

type MediaItem = Track | PlaylistSummary | ArtistSummary;

function isTrack(item: MediaItem): item is Track {
  return item.type === "song" || item.type === "video";
}

function isArtist(item: MediaItem): item is ArtistSummary {
  return item.type === "artist";
}

function subtitle(item: MediaItem): string {
  if (isTrack(item)) {
    return item.artists.map((a) => a.name).join(", ") || "Unknown artist";
  }
  if (isArtist(item)) return item.subscribers || "Artist";
  if (item.type === "album") {
    return [item.author, item.year].filter(Boolean).join(" · ") || "Album";
  }
  return item.author || "Playlist";
}

export function MediaTile({
  item,
  onOpen,
  onPlay,
}: {
  item: MediaItem;
  onOpen: () => void;
  onPlay?: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onOpen}
      className="group relative flex w-[156px] shrink-0 flex-col gap-3 text-left outline-none transition duration-200 hover:-translate-y-0.5 focus-visible:ring-2 focus-visible:ring-[var(--yt-accent)]"
    >
      <div
        className={cn(
          "relative aspect-square overflow-hidden bg-[var(--yt-surface)] shadow-[0_12px_40px_rgba(0,0,0,0.35)]",
          isArtist(item) ? "rounded-full" : "rounded-md",
        )}
      >
        {item.thumbnail ? (
          <Image
            src={item.thumbnail}
            alt=""
            fill
            sizes="156px"
            className="object-cover transition duration-300 group-hover:scale-[1.04]"
            unoptimized
          />
        ) : (
          <div className="flex h-full w-full items-center justify-center text-sm text-[var(--yt-dim)]">
            No art
          </div>
        )}
        {onPlay && (
          <span
            role="presentation"
            onClick={(event) => {
              event.stopPropagation();
              onPlay();
            }}
            className="absolute bottom-2 right-2 flex h-11 w-11 translate-y-2 items-center justify-center rounded-full bg-[var(--yt-accent)] text-[var(--yt-on-accent)] opacity-0 shadow-lg transition duration-200 group-hover:translate-y-0 group-hover:opacity-100"
          >
            <Play className="h-5 w-5 fill-current" />
          </span>
        )}
      </div>
      <div className="min-w-0">
        <div className="truncate text-sm font-semibold text-[var(--yt-text)]">
          {"title" in item ? item.title : item.name}
        </div>
        <div className="truncate text-xs text-[var(--yt-secondary)]">
          {subtitle(item)}
        </div>
      </div>
    </button>
  );
}

export function QuickAccessTile({
  item,
  onOpen,
  onPlay,
}: {
  item: Track | PlaylistSummary;
  onOpen: () => void;
  onPlay: () => void;
}) {
  const title = "title" in item ? item.title : "";
  return (
    <button
      type="button"
      onClick={onOpen}
      className="group flex h-[64px] overflow-hidden rounded-md bg-[var(--yt-surface)] text-left transition hover:bg-[var(--yt-surface-hover)] focus-visible:ring-2 focus-visible:ring-[var(--yt-accent)]"
    >
      <div className="relative h-full w-[64px] shrink-0 bg-[var(--yt-panel)]">
        {item.thumbnail ? (
          <Image
            src={item.thumbnail}
            alt=""
            fill
            sizes="64px"
            className="object-cover"
            unoptimized
          />
        ) : null}
      </div>
      <div className="flex min-w-0 flex-1 items-center justify-between gap-2 px-3">
        <span className="truncate text-sm font-semibold text-[var(--yt-text)]">
          {title}
        </span>
        <span
          role="presentation"
          onClick={(event) => {
            event.stopPropagation();
            onPlay();
          }}
          className="flex h-9 w-9 shrink-0 items-center justify-center rounded-full bg-[var(--yt-accent)] text-[var(--yt-on-accent)] opacity-0 shadow transition group-hover:opacity-100"
        >
          <Play className="h-4 w-4 fill-current" />
        </span>
      </div>
    </button>
  );
}

"use client";

import Image from "next/image";
import { Pause, Play } from "lucide-react";
import { cn, formatDuration } from "@/lib/utils";
import type { Track } from "@/lib/types";

export function TrackRow({
  track,
  index,
  active,
  playing,
  onPlay,
}: {
  track: Track;
  index: number;
  active?: boolean;
  playing?: boolean;
  onPlay: () => void;
}) {
  return (
    <button
      type="button"
      onDoubleClick={onPlay}
      onClick={onPlay}
      className={cn(
        "group grid w-full grid-cols-[40px_minmax(0,1fr)_minmax(0,1fr)_64px] items-center gap-3 rounded-md px-3 py-2 text-left transition",
        active
          ? "bg-[var(--yt-surface-active)] text-[var(--yt-accent)]"
          : "text-[var(--yt-text)] hover:bg-[var(--yt-surface-hover)]",
      )}
    >
      <span className="relative flex h-8 w-8 items-center justify-center text-sm text-[var(--yt-dim)]">
        <span className={cn(playing || active ? "opacity-0" : "group-hover:opacity-0")}>
          {index + 1}
        </span>
        <span
          className={cn(
            "absolute inset-0 flex items-center justify-center text-[var(--yt-text)]",
            playing || active ? "opacity-100" : "opacity-0 group-hover:opacity-100",
          )}
        >
          {playing ? (
            <Pause className="h-4 w-4 fill-current" />
          ) : (
            <Play className="h-4 w-4 fill-current" />
          )}
        </span>
      </span>
      <div className="flex min-w-0 items-center gap-3">
        <div className="relative h-10 w-10 shrink-0 overflow-hidden rounded bg-[var(--yt-surface)]">
          {track.thumbnail ? (
            <Image
              src={track.thumbnail}
              alt=""
              fill
              sizes="40px"
              className="object-cover"
              unoptimized
            />
          ) : null}
        </div>
        <div className="min-w-0">
          <div
            className={cn(
              "truncate text-sm font-medium",
              active ? "text-[var(--yt-accent)]" : "text-[var(--yt-text)]",
            )}
          >
            {track.title}
          </div>
          <div className="truncate text-xs text-[var(--yt-secondary)]">
            {track.artists.map((a) => a.name).join(", ") || "Unknown artist"}
          </div>
        </div>
      </div>
      <div className="hidden truncate text-sm text-[var(--yt-secondary)] sm:block">
        {track.album || "—"}
      </div>
      <div className="text-right text-sm text-[var(--yt-dim)]">
        {track.durationText || formatDuration(track.durationSeconds)}
      </div>
    </button>
  );
}

"use client";

import Image from "next/image";
import {
  Pause,
  Play,
  Repeat,
  Repeat1,
  Shuffle,
  SkipBack,
  SkipForward,
  Volume2,
  VolumeX,
} from "lucide-react";
import { Slider } from "@/components/ui/slider";
import { cn, formatDuration } from "@/lib/utils";
import { usePlayerStore } from "@/lib/player-store";

export function PlayerBar() {
  const track = usePlayerStore((s) => s.current());
  const isPlaying = usePlayerStore((s) => s.isPlaying);
  const progress = usePlayerStore((s) => s.progress);
  const duration = usePlayerStore((s) => s.duration);
  const volume = usePlayerStore((s) => s.volume);
  const shuffle = usePlayerStore((s) => s.shuffle);
  const repeat = usePlayerStore((s) => s.repeat);
  const togglePlay = usePlayerStore((s) => s.togglePlay);
  const next = usePlayerStore((s) => s.next);
  const previous = usePlayerStore((s) => s.previous);
  const seek = usePlayerStore((s) => s.seek);
  const setVolume = usePlayerStore((s) => s.setVolume);
  const toggleShuffle = usePlayerStore((s) => s.toggleShuffle);
  const cycleRepeat = usePlayerStore((s) => s.cycleRepeat);

  const muted = volume === 0;

  return (
    <footer className="grid h-[84px] shrink-0 grid-cols-[minmax(0,1.1fr)_minmax(0,1.4fr)_minmax(0,1fr)] items-center gap-4 border-t border-[var(--yt-outline)] bg-[var(--yt-panel)] px-4">
      <div className="flex min-w-0 items-center gap-3">
        <div className="relative h-14 w-14 shrink-0 overflow-hidden rounded bg-[var(--yt-surface)]">
          {track?.thumbnail ? (
            <Image
              src={track.thumbnail}
              alt=""
              fill
              sizes="56px"
              className="object-cover"
              unoptimized
            />
          ) : null}
        </div>
        <div className="min-w-0">
          <div className="truncate text-sm font-semibold text-[var(--yt-text)]">
            {track?.title || "Nothing playing"}
          </div>
          <div className="truncate text-xs text-[var(--yt-secondary)]">
            {track
              ? track.artists.map((a) => a.name).join(", ") || "Unknown artist"
              : "Pick something from Home or Search"}
          </div>
        </div>
      </div>

      <div className="flex flex-col items-center gap-1.5">
        <div className="flex items-center gap-3">
          <button
            type="button"
            aria-label="Shuffle"
            onClick={toggleShuffle}
            className={cn(
              "rounded p-1.5 transition",
              shuffle
                ? "text-[var(--yt-accent)]"
                : "text-[var(--yt-dim)] hover:text-[var(--yt-text)]",
            )}
          >
            <Shuffle className="h-4 w-4" />
          </button>
          <button
            type="button"
            aria-label="Previous"
            onClick={previous}
            className="rounded p-1.5 text-[var(--yt-text)] transition hover:text-white"
          >
            <SkipBack className="h-5 w-5 fill-current" />
          </button>
          <button
            type="button"
            aria-label={isPlaying ? "Pause" : "Play"}
            onClick={togglePlay}
            disabled={!track}
            className="flex h-10 w-10 items-center justify-center rounded-full bg-[var(--yt-text)] text-[var(--yt-window)] transition hover:scale-105 disabled:opacity-40"
          >
            {isPlaying ? (
              <Pause className="h-5 w-5 fill-current" />
            ) : (
              <Play className="h-5 w-5 fill-current" />
            )}
          </button>
          <button
            type="button"
            aria-label="Next"
            onClick={next}
            className="rounded p-1.5 text-[var(--yt-text)] transition hover:text-white"
          >
            <SkipForward className="h-5 w-5 fill-current" />
          </button>
          <button
            type="button"
            aria-label="Repeat"
            onClick={cycleRepeat}
            className={cn(
              "rounded p-1.5 transition",
              repeat !== "off"
                ? "text-[var(--yt-accent)]"
                : "text-[var(--yt-dim)] hover:text-[var(--yt-text)]",
            )}
          >
            {repeat === "one" ? (
              <Repeat1 className="h-4 w-4" />
            ) : (
              <Repeat className="h-4 w-4" />
            )}
          </button>
        </div>
        <div className="flex w-full max-w-[520px] items-center gap-2">
          <span className="w-10 text-right text-[11px] tabular-nums text-[var(--yt-dim)]">
            {formatDuration(progress)}
          </span>
          <Slider
            value={[Math.min(progress, duration || progress)]}
            max={Math.max(duration || 1, 1)}
            step={1}
            disabled={!track}
            onValueChange={([value]) => seek(value)}
            className="flex-1"
          />
          <span className="w-10 text-[11px] tabular-nums text-[var(--yt-dim)]">
            {formatDuration(duration)}
          </span>
        </div>
      </div>

      <div className="flex items-center justify-end gap-2">
        <button
          type="button"
          aria-label={muted ? "Unmute" : "Mute"}
          onClick={() => setVolume(muted ? 0.85 : 0)}
          className="rounded p-1.5 text-[var(--yt-secondary)] hover:text-[var(--yt-text)]"
        >
          {muted ? (
            <VolumeX className="h-4 w-4" />
          ) : (
            <Volume2 className="h-4 w-4" />
          )}
        </button>
        <Slider
          value={[volume * 100]}
          max={100}
          step={1}
          onValueChange={([value]) => setVolume(value / 100)}
          className="w-28"
        />
      </div>
    </footer>
  );
}

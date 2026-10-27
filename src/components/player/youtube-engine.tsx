"use client";

import { useEffect, useRef } from "react";
import { usePlayerStore } from "@/lib/player-store";

declare global {
  interface Window {
    YT?: {
      Player: new (
        element: HTMLElement | string,
        options: Record<string, unknown>,
      ) => YtPlayer;
      PlayerState: {
        PLAYING: number;
        PAUSED: number;
        ENDED: number;
        BUFFERING: number;
      };
    };
    onYouTubeIframeAPIReady?: () => void;
  }
}

interface YtPlayer {
  destroy: () => void;
  loadVideoById: (id: string) => void;
  playVideo: () => void;
  pauseVideo: () => void;
  seekTo: (seconds: number, allowSeekAhead: boolean) => void;
  setVolume: (volume: number) => void;
  getCurrentTime: () => number;
  getDuration: () => number;
  getPlayerState: () => number;
}

let apiPromise: Promise<void> | null = null;

function loadYoutubeApi(): Promise<void> {
  if (typeof window === "undefined") return Promise.resolve();
  if (window.YT?.Player) return Promise.resolve();
  if (apiPromise) return apiPromise;
  apiPromise = new Promise((resolve) => {
    const existing = document.querySelector<HTMLScriptElement>(
      'script[src="https://www.youtube.com/iframe_api"]',
    );
    if (!existing) {
      const script = document.createElement("script");
      script.src = "https://www.youtube.com/iframe_api";
      document.body.appendChild(script);
    }
    const previous = window.onYouTubeIframeAPIReady;
    window.onYouTubeIframeAPIReady = () => {
      previous?.();
      resolve();
    };
  });
  return apiPromise;
}

export function PlaybackEngine() {
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const ytHostRef = useRef<HTMLDivElement | null>(null);
  const ytPlayerRef = useRef<YtPlayer | null>(null);
  const seekGuard = useRef(false);

  const current = usePlayerStore((s) => s.current());
  const isPlaying = usePlayerStore((s) => s.isPlaying);
  const volume = usePlayerStore((s) => s.volume);
  const progress = usePlayerStore((s) => s.progress);
  const streamUrl = usePlayerStore((s) => s.streamUrl());
  const setProgress = usePlayerStore((s) => s.setProgress);
  const next = usePlayerStore((s) => s.next);

  const isDemo = Boolean(streamUrl);
  const videoId = current && !isDemo ? current.id : null;

  useEffect(() => {
    if (!videoId || !ytHostRef.current) return;
    let cancelled = false;

    loadYoutubeApi().then(() => {
      if (cancelled || !window.YT || !ytHostRef.current) return;
      if (ytPlayerRef.current) {
        ytPlayerRef.current.loadVideoById(videoId);
        return;
      }
      ytPlayerRef.current = new window.YT.Player(ytHostRef.current, {
        height: "1",
        width: "1",
        videoId,
        playerVars: {
          autoplay: 1,
          controls: 0,
          disablekb: 1,
          fs: 0,
          modestbranding: 1,
          playsinline: 1,
          rel: 0,
        },
        events: {
          onStateChange: (event: { data: number }) => {
            if (event.data === window.YT?.PlayerState.ENDED) next();
          },
          onReady: (event: { target: YtPlayer }) => {
            event.target.setVolume(Math.round(volume * 100));
            if (usePlayerStore.getState().isPlaying) event.target.playVideo();
          },
        },
      });
    });

    return () => {
      cancelled = true;
    };
  }, [videoId, next, volume]);

  useEffect(() => {
    const player = ytPlayerRef.current;
    if (!player || isDemo) return;
    if (isPlaying) player.playVideo();
    else player.pauseVideo();
  }, [isPlaying, isDemo, videoId]);

  useEffect(() => {
    ytPlayerRef.current?.setVolume(Math.round(volume * 100));
    if (audioRef.current) audioRef.current.volume = volume;
  }, [volume]);

  useEffect(() => {
    if (!isDemo || !audioRef.current) return;
    const audio = audioRef.current;
    if (streamUrl && audio.src !== streamUrl) {
      audio.src = streamUrl;
      audio.load();
    }
    if (isPlaying) void audio.play().catch(() => undefined);
    else audio.pause();
  }, [isDemo, streamUrl, isPlaying, current?.id]);

  useEffect(() => {
    if (!seekGuard.current) return;
    seekGuard.current = false;
    if (isDemo && audioRef.current) {
      audioRef.current.currentTime = progress;
    } else {
      ytPlayerRef.current?.seekTo(progress, true);
    }
  }, [progress, isDemo]);

  useEffect(() => {
    const originalSeek = usePlayerStore.getState().seek;
    usePlayerStore.setState({
      seek: (seconds: number) => {
        seekGuard.current = true;
        originalSeek(seconds);
      },
    });
  }, []);

  useEffect(() => {
    const id = window.setInterval(() => {
      if (isDemo && audioRef.current) {
        setProgress(audioRef.current.currentTime, audioRef.current.duration || undefined);
        return;
      }
      const player = ytPlayerRef.current;
      if (!player || typeof player.getCurrentTime !== "function") return;
      try {
        setProgress(player.getCurrentTime(), player.getDuration() || undefined);
      } catch {
        // Player may not be ready yet.
      }
    }, 400);
    return () => window.clearInterval(id);
  }, [isDemo, setProgress]);

  return (
    <>
      <audio
        ref={audioRef}
        preload="metadata"
        onEnded={() => next()}
        className="hidden"
      />
      <div
        ref={ytHostRef}
        className="pointer-events-none absolute h-px w-px overflow-hidden opacity-0"
        aria-hidden
      />
    </>
  );
}

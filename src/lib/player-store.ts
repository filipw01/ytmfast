"use client";

import { create } from "zustand";
import type { Track } from "./types";
import { demoAudioUrl } from "./demo-data";

export type ViewId =
  | "home"
  | "search"
  | "library"
  | "explore"
  | { kind: "playlist"; id: string; title?: string };

interface PlayerState {
  queue: Track[];
  index: number;
  isPlaying: boolean;
  progress: number;
  duration: number;
  volume: number;
  shuffle: boolean;
  repeat: "off" | "all" | "one";
  demoMode: boolean;
  view: ViewId;
  searchQuery: string;
  setView: (view: ViewId) => void;
  setSearchQuery: (query: string) => void;
  setDemoMode: (demo: boolean) => void;
  playTrack: (track: Track, queue?: Track[]) => void;
  playQueue: (tracks: Track[], startIndex?: number) => void;
  togglePlay: () => void;
  pause: () => void;
  resume: () => void;
  next: () => void;
  previous: () => void;
  seek: (seconds: number) => void;
  setProgress: (seconds: number, duration?: number) => void;
  setVolume: (volume: number) => void;
  toggleShuffle: () => void;
  cycleRepeat: () => void;
  current: () => Track | null;
  streamUrl: () => string | null;
}

export const usePlayerStore = create<PlayerState>((set, get) => ({
  queue: [],
  index: -1,
  isPlaying: false,
  progress: 0,
  duration: 0,
  volume: 0.85,
  shuffle: false,
  repeat: "off",
  demoMode: false,
  view: "home",
  searchQuery: "",

  setView: (view) => set({ view }),
  setSearchQuery: (searchQuery) => set({ searchQuery }),
  setDemoMode: (demoMode) => set({ demoMode }),

  playTrack: (track, queue) => {
    const nextQueue = queue?.length ? queue : [track];
    const index = Math.max(
      0,
      nextQueue.findIndex((t) => t.id === track.id),
    );
    set({
      queue: nextQueue,
      index: index === -1 ? 0 : index,
      isPlaying: true,
      progress: 0,
      duration: track.durationSeconds ?? 0,
    });
  },

  playQueue: (tracks, startIndex = 0) => {
    if (!tracks.length) return;
    set({
      queue: tracks,
      index: Math.min(Math.max(0, startIndex), tracks.length - 1),
      isPlaying: true,
      progress: 0,
      duration: tracks[startIndex]?.durationSeconds ?? 0,
    });
  },

  togglePlay: () => {
    const { queue, index, isPlaying } = get();
    if (index < 0 || !queue[index]) return;
    set({ isPlaying: !isPlaying });
  },

  pause: () => set({ isPlaying: false }),
  resume: () => {
    if (get().index >= 0) set({ isPlaying: true });
  },

  next: () => {
    const { queue, index, shuffle, repeat } = get();
    if (!queue.length) return;
    if (repeat === "one") {
      set({ progress: 0, isPlaying: true });
      return;
    }
    let nextIndex = index + 1;
    if (shuffle) {
      nextIndex = Math.floor(Math.random() * queue.length);
    } else if (nextIndex >= queue.length) {
      if (repeat === "all") nextIndex = 0;
      else {
        set({ isPlaying: false });
        return;
      }
    }
    set({
      index: nextIndex,
      progress: 0,
      duration: queue[nextIndex]?.durationSeconds ?? 0,
      isPlaying: true,
    });
  },

  previous: () => {
    const { queue, index, progress } = get();
    if (!queue.length) return;
    if (progress > 3) {
      set({ progress: 0 });
      return;
    }
    const prevIndex = index <= 0 ? queue.length - 1 : index - 1;
    set({
      index: prevIndex,
      progress: 0,
      duration: queue[prevIndex]?.durationSeconds ?? 0,
      isPlaying: true,
    });
  },

  seek: (seconds) => set({ progress: Math.max(0, seconds) }),
  setProgress: (progress, duration) =>
    set((state) => ({
      progress,
      duration: duration ?? state.duration,
    })),
  setVolume: (volume) => set({ volume: Math.min(1, Math.max(0, volume)) }),
  toggleShuffle: () => set((s) => ({ shuffle: !s.shuffle })),
  cycleRepeat: () =>
    set((s) => ({
      repeat: s.repeat === "off" ? "all" : s.repeat === "all" ? "one" : "off",
    })),

  current: () => {
    const { queue, index } = get();
    return queue[index] ?? null;
  },

  streamUrl: () => {
    const track = get().current();
    if (!track) return null;
    return demoAudioUrl(track.id);
  },
}));

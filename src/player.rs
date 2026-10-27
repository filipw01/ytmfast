//! Local playback of YouTube Music.
//!
//! The engine owns one audio thread, which decodes a downloaded stream with
//! symphonia and plays it through [`crate::sink::RodioSink`], and the queue
//! the interface drives: the context's tracks in play order, the songs added
//! by hand, shuffle and repeat. Audio comes from `rusty_ytdl`, which resolves
//! a video id to its streams; the best AAC-in-MP4 audio stream is downloaded
//! whole (a song is a few megabytes), kept in the cache folder, and decoded
//! from memory, so seeking is exact and the next song can be fetched while
//! this one plays. Everything the audio thread does is folded into a
//! [`LocalState`] snapshot that is pushed to the interface whenever something
//! changed.
//!
//! Playback is local only: there is no Connect device, so `Transfer` has
//! nothing to take over and the rootlist is empty.

use std::collections::{HashMap, VecDeque};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, Decoder as CodecDecoder, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::{Time, TimeBase};

use crate::api::models::ArtistRef;
use crate::resample::Resampler;
use crate::sink::{
    AudioControl, DeviceHook, ErrorHook, NUM_CHANNELS, Processing, RodioSink, SAMPLE_RATE,
    SharedVolume,
};
use crate::vis::AudioTap;

/// What the engine signs in with: a YouTube Music browser cookie, or, for
/// older callers, a password kept in the same field.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials {
    pub username: String,
    /// The browser cookie or auth blob. Never logged.
    pub cookie: String,
}

impl Credentials {
    pub fn with_cookie(username: impl Into<String>, cookie: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            cookie: cookie.into(),
        }
    }

    /// Kept under its old name for callers that sign in with a password.
    /// YouTube Music has no password sign in, so the secret is stored in the
    /// cookie field and is only sent to YouTube if it is a cookie string.
    pub fn with_password(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self::with_cookie(username, password)
    }

    /// The cookie, when it is one a request can carry (`name=value; ...`).
    /// A Google OAuth token is never sent as a cookie.
    fn request_cookie(&self) -> Option<String> {
        let cookie = self.cookie.trim();
        (cookie.contains('=')
            && !cookie.contains(['\r', '\n'])
            && !crate::auth::is_oauth_blob(cookie))
        .then(|| cookie.to_string())
    }
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("username", &self.username)
            .field("cookie", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct EngineConfig {
    pub device_name: String,
    /// The audio quality to prefer, as the nearest stream's bitrate.
    pub bitrate_kbps: u16,
    /// Plays quiet and loud songs at the level YouTube measured for them.
    pub normalisation: bool,
    pub autoplay: bool,
    pub gapless: bool,
    /// The saved output, an id or an older build's name (`crate::sink::Saved`).
    pub audio_device: Option<String>,
    pub initial_volume: u16,
    pub volume_dir: PathBuf,
    pub audio_cache_dir: Option<PathBuf>,
    pub audio_cache_limit: Option<u64>,
    /// Output buffer length in milliseconds.
    pub buffer_ms: u32,
    pub tap: Arc<AudioTap>,
    /// The equalizer's settings, shared with the window that sets them.
    pub eq: crate::eq::SharedEq,
    /// Proxy used by the API client.
    pub proxy: crate::settings::ProxyConfig,
    /// Unused: YouTube Music has no local files.
    pub local_files: Option<PathBuf>,
}

impl EngineConfig {
    /// A stable device id derived from the name, so this computer is
    /// recognised across restarts.
    pub fn device_id(&self) -> String {
        hex(&Sha1::digest(self.device_name.as_bytes()))
    }

    /// The folder downloaded songs are kept in, created if need be.
    pub fn open_cache(&self) -> Result<PathBuf> {
        let dir = self
            .audio_cache_dir
            .clone()
            .unwrap_or_else(|| self.volume_dir.join("audio"));
        std::fs::create_dir_all(&dir).context("unable to open the playback cache")?;
        Ok(dir)
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Playback {
    #[default]
    Stopped,
    Loading,
    Playing,
    Paused,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RepeatMode {
    #[default]
    Off,
    Context,
    Track,
}

impl RepeatMode {
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::Context,
            Self::Context => Self::Track,
            Self::Track => Self::Off,
        }
    }

    pub fn api_name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Context => "context",
            Self::Track => "track",
        }
    }

    pub fn from_api(name: &str) -> Self {
        match name {
            "context" => Self::Context,
            "track" => Self::Track,
            _ => Self::Off,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LocalTrack {
    pub uri: String,
    pub title: String,
    pub artists: Vec<ArtistRef>,
    pub album: String,
    pub art_url: Option<String>,
    pub art_small_url: Option<String>,
    pub duration_ms: u32,
    pub is_episode: bool,
}

impl LocalTrack {
    pub fn artist_names(&self) -> String {
        crate::api::models::join_names(self.artists.iter().map(|artist| artist.name.as_str()))
    }
}

/// The engine error for a song YouTube refused to hand out (a sign-in or
/// bot check, or a stream URL that was turned down). The app answers it with
/// a banner explaining what still works, rather than the text itself.
pub const AUDIO_KEY_REFUSED: &str = "YouTube Music refused the audio stream. Try again later";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LocalState {
    pub playback: Playback,
    pub track: Option<LocalTrack>,
    pub position_ms: u32,
    /// When `position_ms` was observed; `None` while not advancing.
    pub position_at: Option<Instant>,
    pub volume: u16,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    /// The engine is up and signed in.
    pub connected: bool,
    pub username: String,
    pub active_client: String,
    pub error: Option<String>,
    pub seek_sequence: u64,
    /// The engine is fetching a track right now, even when the previous
    /// track's `playback` still reads `Playing` so its controls stay visible
    /// through the swap.
    pub loading: bool,
    /// A newly loaded track, including another play of the same URI.
    pub track_sequence: u64,
    /// Another play of the same track started, and its start position has
    /// not arrived yet. The track looks unchanged, so the `Playing` or
    /// `Paused` that brings the position counts as a seek for media
    /// controls, which would otherwise count on past the end (#587).
    pub replay_pending: bool,
}

/// What local playback was doing when the engine ended, so it can pick it
/// up again after reconnecting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interrupted {
    pub uri: String,
    pub position_ms: u32,
    /// Playing or loading, as opposed to paused.
    pub playing: bool,
}

impl LocalState {
    /// The track and position to come back to, if something was on.
    pub fn interrupted(&self) -> Option<Interrupted> {
        let track = self.track.as_ref()?;
        if self.playback == Playback::Stopped {
            return None;
        }
        Some(Interrupted {
            uri: track.uri.clone(),
            position_ms: self.position_now(),
            playing: matches!(self.playback, Playback::Playing | Playback::Loading),
        })
    }

    /// The position now, interpolated from the last report while playing.
    pub fn position_now(&self) -> u32 {
        match (self.playback, self.position_at) {
            (Playback::Playing, Some(at)) => {
                let elapsed = at.elapsed().as_millis() as u32;
                let limit = self
                    .track
                    .as_ref()
                    .map_or(u32::MAX, |track| track.duration_ms.max(self.position_ms));
                self.position_ms.saturating_add(elapsed).min(limit)
            }
            _ => self.position_ms,
        }
    }

    pub fn is_active(&self) -> bool {
        self.track.is_some() && self.playback != Playback::Stopped
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoadSpec {
    /// The playlist or album the songs come from. Playback needs `uris`:
    /// the caller resolves a context into its songs.
    pub context_uri: Option<String>,
    pub uris: Vec<String>,
    pub offset_uri: Option<String>,
    pub offset_index: Option<u32>,
    pub position_ms: u32,
    pub play: bool,
    pub shuffle: Option<bool>,
    /// Explicit repeat preference for a new load. Otherwise the engine's
    /// current preference is kept.
    pub repeat: Option<RepeatMode>,
    /// Unused: there is no autoplay station to follow a context with.
    pub autoplay: bool,
}

/// The queue and position of local playback, enough to start it again
/// where it left off.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LocalPlaybackSnapshot {
    /// The song that was on.
    pub uri: String,
    pub position_ms: u32,
    pub playing: bool,
    /// The context's songs in their own order.
    pub queue: Vec<String>,
    /// Where the song on is in `queue`.
    pub queue_index: usize,
    /// The songs added by hand, in the order they play.
    pub user_queue: Vec<String>,
    pub shuffle: bool,
    pub repeat: RepeatMode,
}

impl LocalPlaybackSnapshot {
    /// The same queue, with the song where it has got to now.
    pub fn at_position(&self, position_ms: u32, playing: bool) -> Self {
        Self {
            position_ms,
            playing,
            ..self.clone()
        }
    }
}

/// How to pick playback up again: the whole queue as it was, or just the
/// song, for a replacement after an audio-settings change.
#[derive(Clone, Debug)]
pub enum PlaybackResume {
    Snapshot(LocalPlaybackSnapshot),
    Track(LoadSpec),
}

#[derive(Clone, Debug, PartialEq)]
pub enum PlayerCommand {
    Toggle,
    Next,
    Previous,
    /// Remove manually queued tracks and keep context tracks.
    ClearQueue,
    /// Queue a song after the ones already queued.
    AddToQueue(String),
    Seek(u32),
    /// The volume to keep: applied at once and reported.
    Volume(u16),
    /// The slider mid-drag: applied at once, nothing reported.
    VolumePreview(u16),
    Shuffle(bool),
    Repeat(RepeatMode),
    Load(LoadSpec),
    /// Playback is local only, so there is nothing to take over: succeeds
    /// without doing anything.
    Transfer,
}

#[allow(clippy::large_enum_variant)]
pub enum EngineEvent {
    State(LocalState),
    SessionEnded,
    /// The output opened on the device an older build saved by `name`;
    /// the setting holds its `id` from now on.
    AudioDevice {
        name: String,
        id: String,
    },
}

pub type Notify = Arc<dyn Fn(EngineEvent) + Send + Sync>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// ---------------------------------------------------------------------------
// Track URIs
// ---------------------------------------------------------------------------

/// The URI the app uses for a YouTube Music song.
pub fn track_uri(video_id: &str) -> String {
    format!("ytmusic:track:{video_id}")
}

fn is_video_id(text: &str) -> bool {
    text.len() == 11
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// The video id a track URI names: `ytmusic:track:ID`, `ytm:ID`,
/// `ytm:track:ID`, a YouTube or YouTube Music link, or a bare id.
pub fn video_id_from_uri(uri: &str) -> Result<String> {
    let uri = uri.trim();
    if uri.starts_with("spotify:") {
        bail!("Spotify items can't be played here: {uri}");
    }
    let candidate = if let Some(rest) = uri
        .strip_prefix("ytmusic:track:")
        .or_else(|| uri.strip_prefix("ytm:track:"))
        .or_else(|| uri.strip_prefix("ytmusic:"))
        .or_else(|| uri.strip_prefix("ytm:"))
    {
        rest
    } else if let Some((_, query)) = uri
        .split_once("watch?")
        .filter(|_| uri.contains("youtube."))
    {
        query
            .split('&')
            .find_map(|pair| pair.strip_prefix("v="))
            .unwrap_or_default()
    } else if let Some((_, rest)) = uri.split_once("youtu.be/") {
        rest.split(['?', '&', '/']).next().unwrap_or_default()
    } else {
        uri
    };
    if is_video_id(candidate) {
        Ok(candidate.to_string())
    } else {
        bail!("not a YouTube Music track: {uri}")
    }
}

// ---------------------------------------------------------------------------
// State events
// ---------------------------------------------------------------------------

/// What happened to playback, as the audio thread and the queue report it.
#[derive(Debug)]
enum Event {
    Loading {
        position_ms: u32,
    },
    Playing {
        position_ms: u32,
    },
    Paused {
        position_ms: u32,
    },
    Stopped,
    Position {
        position_ms: u32,
    },
    Seeked {
        position_ms: u32,
    },
    TrackChanged(Box<LocalTrack>),
    /// A load or the stream failed: the message says why.
    Failed(String),
    /// An output problem that leaves playback as it is.
    Error(String),
    Shuffle(bool),
    Repeat(RepeatMode),
}

fn set<T: PartialEq>(target: &mut T, value: T) -> bool {
    if *target == value {
        false
    } else {
        *target = value;
        true
    }
}

/// Reports a replay's start position as a seek, once.
fn start_replay(state: &mut LocalState) -> bool {
    if std::mem::take(&mut state.replay_pending) {
        state.seek_sequence = state.seek_sequence.wrapping_add(1);
        true
    } else {
        false
    }
}

/// Folds `event` into `state`; whether anything the interface shows changed.
fn apply_event(state: &mut LocalState, event: Event) -> bool {
    match event {
        Event::Stopped => {
            let mut changed = set(&mut state.playback, Playback::Stopped);
            changed |= set(&mut state.loading, false);
            changed |= set(&mut state.position_ms, 0);
            changed |= set(&mut state.position_at, None);
            changed
        }
        Event::Loading { position_ms } => {
            let mut changed = if state.playback == Playback::Stopped {
                set(&mut state.playback, Playback::Loading)
            } else {
                false
            };
            changed |= set(&mut state.loading, true);
            changed |= set(&mut state.position_ms, position_ms);
            changed |= set(&mut state.position_at, None);
            changed |= set(&mut state.error, None);
            changed
        }
        Event::Playing { position_ms } => {
            set(&mut state.playback, Playback::Playing);
            set(&mut state.position_ms, position_ms);
            state.position_at = Some(Instant::now());
            state.loading = false;
            start_replay(state);
            true
        }
        Event::Paused { position_ms } => {
            let mut changed = set(&mut state.playback, Playback::Paused);
            changed |= set(&mut state.loading, false);
            changed |= set(&mut state.position_ms, position_ms);
            changed |= set(&mut state.position_at, None);
            changed | start_replay(state)
        }
        Event::Position { position_ms } => {
            state.position_ms = position_ms;
            if state.playback == Playback::Playing {
                state.position_at = Some(Instant::now());
            }
            true
        }
        Event::Seeked { position_ms } => {
            state.position_ms = position_ms;
            if state.playback == Playback::Playing {
                state.position_at = Some(Instant::now());
            }
            state.seek_sequence = state.seek_sequence.wrapping_add(1);
            true
        }
        Event::TrackChanged(track) => {
            state.replay_pending = state
                .track
                .as_ref()
                .is_some_and(|previous| previous.uri == track.uri);
            state.track = Some(*track);
            state.error = None;
            // Every loaded track counts, including a repeat whose URI and
            // metadata are identical to the previous play.
            state.track_sequence = state.track_sequence.wrapping_add(1);
            true
        }
        Event::Failed(message) => {
            // A failed load never reaches Playing, so nothing else would
            // turn the spinner off, and nothing is sounding any more.
            let mut changed = set(&mut state.error, Some(message));
            changed |= set(&mut state.loading, false);
            changed |= set(&mut state.playback, Playback::Stopped);
            changed |= set(&mut state.position_at, None);
            changed
        }
        Event::Error(message) => set(&mut state.error, Some(message)),
        Event::Shuffle(shuffle) => set(&mut state.shuffle, shuffle),
        Event::Repeat(mode) => set(&mut state.repeat, mode),
    }
}

/// The engine is gone. A song playing or paused stays as it is until the
/// next engine takes it over or the player is stopped (#268); a shutdown
/// asked for stops it.
fn session_ended(state: &mut LocalState, shut_down: bool) {
    state.connected = false;
    if shut_down || !matches!(state.playback, Playback::Playing | Playback::Paused) {
        state.playback = Playback::Stopped;
        state.loading = false;
        state.position_at = None;
    }
}

fn command_interrupts_audio(state: &LocalState, command: &PlayerCommand) -> bool {
    state.playback == Playback::Playing
        && matches!(
            command,
            PlayerCommand::Next | PlayerCommand::Previous | PlayerCommand::Load(_)
        )
}

// ---------------------------------------------------------------------------
// The queue
// ---------------------------------------------------------------------------

/// What plays now, what plays next, and in which order.
#[derive(Debug, Default)]
struct Queue {
    /// The context's songs in their own order.
    list: Vec<String>,
    /// Indices into `list` in play order: the identity, or a shuffle with
    /// the song that was chosen first.
    order: Vec<usize>,
    /// Where in `order` the context song on (or last on) is.
    pos: usize,
    /// The song loaded now. It is not the context's when it came from the
    /// manual queue.
    current: Option<String>,
    /// Songs added by hand, which play before the context goes on.
    user: VecDeque<String>,
    shuffle: bool,
    repeat: RepeatMode,
    /// Songs in a row that would not load.
    failures: u8,
}

impl Queue {
    fn set_context(&mut self, list: Vec<String>, start: usize) {
        let start = start.min(list.len().saturating_sub(1));
        self.list = list;
        self.arrange(start);
        self.current = self.list.get(start).cloned();
        self.failures = 0;
    }

    /// Lays out the play order with list item `first` where playback is.
    fn arrange(&mut self, first: usize) {
        self.order = (0..self.list.len()).collect();
        if self.shuffle && self.order.len() > 1 {
            self.order.retain(|&index| index != first);
            self.order.shuffle(&mut rand::rng());
            self.order.insert(0, first);
            self.pos = 0;
        } else {
            self.pos = first;
        }
    }

    fn context_index(&self) -> Option<usize> {
        self.order.get(self.pos).copied()
    }

    fn context_uri(&self) -> Option<&String> {
        self.context_index().and_then(|index| self.list.get(index))
    }

    fn set_shuffle(&mut self, shuffle: bool) {
        if self.shuffle == shuffle {
            return;
        }
        let here = self.context_index();
        self.shuffle = shuffle;
        if let Some(here) = here {
            self.arrange(here);
        }
    }

    /// The song after this one: the manual queue first, then the context.
    /// `natural` is the song having ended on its own, which repeat-one
    /// answers by playing it again; a skip always moves on.
    fn next(&mut self, natural: bool) -> Option<String> {
        if natural && self.repeat == RepeatMode::Track && self.current.is_some() {
            return self.current.clone();
        }
        if let Some(uri) = self.user.pop_front() {
            self.current = Some(uri.clone());
            return Some(uri);
        }
        if self.order.is_empty() {
            return None;
        }
        if self.pos + 1 < self.order.len() {
            self.pos += 1;
        } else if self.repeat == RepeatMode::Context {
            if self.shuffle {
                self.order.shuffle(&mut rand::rng());
            }
            self.pos = 0;
        } else {
            return None;
        }
        self.current = self.context_uri().cloned();
        self.current.clone()
    }

    /// The song before this one, or `None` when there is none to go back to.
    fn previous(&mut self) -> Option<String> {
        if let Some(context) = self.context_uri().cloned()
            && self.current.as_ref() != Some(&context)
        {
            self.current = Some(context.clone());
            return Some(context);
        }
        if self.pos > 0 {
            self.pos -= 1;
        } else if self.repeat == RepeatMode::Context && self.order.len() > 1 {
            self.pos = self.order.len() - 1;
        } else {
            return None;
        }
        self.current = self.context_uri().cloned();
        self.current.clone()
    }

    /// The song `next` would answer with, if it is known without choosing.
    fn peek_next(&self) -> Option<String> {
        if self.repeat == RepeatMode::Track {
            return None;
        }
        if let Some(uri) = self.user.front() {
            return Some(uri.clone());
        }
        if self.pos + 1 < self.order.len() {
            return self.list.get(self.order[self.pos + 1]).cloned();
        }
        if self.repeat == RepeatMode::Context && !self.shuffle {
            return self
                .order
                .first()
                .and_then(|&index| self.list.get(index))
                .cloned();
        }
        None
    }

    fn snapshot(&self, position_ms: u32, playing: bool) -> Option<LocalPlaybackSnapshot> {
        Some(LocalPlaybackSnapshot {
            uri: self.current.clone()?,
            position_ms,
            playing,
            queue: self.list.clone(),
            queue_index: self.context_index().unwrap_or(0),
            user_queue: self.user.iter().cloned().collect(),
            shuffle: self.shuffle,
            repeat: self.repeat,
        })
    }

    fn restore(&mut self, snapshot: &LocalPlaybackSnapshot) {
        self.shuffle = snapshot.shuffle;
        self.repeat = snapshot.repeat;
        self.list = snapshot.queue.clone();
        self.arrange(snapshot.queue_index.min(self.list.len().saturating_sub(1)));
        self.user = snapshot.user_queue.iter().cloned().collect();
        self.current = Some(snapshot.uri.clone());
        self.failures = 0;
    }
}

// ---------------------------------------------------------------------------
// Fetching a song
// ---------------------------------------------------------------------------

/// The browser YouTube is told it is talking to, for the stream downloads.
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/87.0.4280.101 Safari/537.36";

/// The most one range request asks for.
const DOWNLOAD_CHUNK: u64 = 8 * 1024 * 1024;

/// How long resolving a video may take.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(40);

/// Songs kept decoded-ready in memory, so a repeat or a skip back is instant.
const MEMORY_SONGS: usize = 4;

/// What is known about a song apart from its URI.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Meta {
    title: String,
    artist: String,
    artist_id: String,
    art_url: Option<String>,
    art_small_url: Option<String>,
    duration_ms: u32,
    loudness_db: Option<f64>,
}

impl Meta {
    fn track(&self, uri: &str) -> LocalTrack {
        let artists = if self.artist.is_empty() {
            Vec::new()
        } else {
            let id = (!self.artist_id.is_empty()).then(|| self.artist_id.clone());
            vec![ArtistRef {
                uri: id.as_ref().map(|id| format!("ytmusic:artist:{id}")),
                id,
                name: self.artist.clone(),
            }]
        };
        LocalTrack {
            uri: uri.to_string(),
            title: self.title.clone(),
            artists,
            album: String::new(),
            art_url: self.art_url.clone(),
            art_small_url: self.art_small_url.clone(),
            duration_ms: self.duration_ms,
            is_episode: false,
        }
    }
}

/// A song ready to decode.
struct Prepared {
    meta: Meta,
    bytes: Arc<Vec<u8>>,
}

/// Why a song would not load.
#[derive(Debug)]
enum LoadError {
    /// YouTube would not hand the stream out.
    Refused,
    /// There is no stream to play: removed, private, or in a format the
    /// player has no decoder for.
    Unavailable(String),
    Network(String),
}

impl LoadError {
    fn reason(&self) -> String {
        match self {
            Self::Refused => "the stream was refused".into(),
            Self::Unavailable(reason) | Self::Network(reason) => reason.clone(),
        }
    }

    fn message(&self, uri: &str) -> String {
        match self {
            Self::Refused => AUDIO_KEY_REFUSED.to_string(),
            Self::Unavailable(_) => format!("This item isn't available: {uri}"),
            Self::Network(error) => format!("Couldn't load {uri}: {error}"),
        }
    }
}

impl From<rusty_ytdl::VideoError> for LoadError {
    fn from(error: rusty_ytdl::VideoError) -> Self {
        use rusty_ytdl::VideoError;
        match error {
            VideoError::VideoNotFound
            | VideoError::VideoIsPrivate
            | VideoError::VideoSourceNotFound
            | VideoError::FormatNotFound => Self::Unavailable(error.to_string()),
            VideoError::VideoPlayerResponseError(_) => Self::Refused,
            other => Self::Network(other.to_string()),
        }
    }
}

impl From<reqwest::Error> for LoadError {
    fn from(error: reqwest::Error) -> Self {
        if error.status() == Some(reqwest::StatusCode::FORBIDDEN) {
            Self::Refused
        } else {
            Self::Network(error.to_string())
        }
    }
}

/// The audio stream to play: AAC in MP4, which symphonia decodes, nearest
/// the bitrate asked for. WebM streams (Opus, Vorbis) have no decoder here.
fn pick_format(
    formats: &[rusty_ytdl::VideoFormat],
    target_bps: u64,
) -> Option<&rusty_ytdl::VideoFormat> {
    let bitrate =
        |format: &rusty_ytdl::VideoFormat| format.average_bitrate.unwrap_or(format.bitrate);
    formats
        .iter()
        .filter(|format| {
            format.has_audio
                && !format.has_video
                && !format.is_live
                && !format.is_hls
                && !format.url.is_empty()
                && format.mime_type.container == "mp4"
                && format
                    .mime_type
                    .audio_codec
                    .as_deref()
                    .is_none_or(|codec| codec.starts_with("mp4a"))
        })
        .min_by_key(|format| {
            let rate = bitrate(format);
            (rate.abs_diff(target_bps), std::cmp::Reverse(rate))
        })
}

/// The channel name without YouTube's " - Topic" suffix for auto-generated
/// artist channels.
fn artist_name(channel: &str) -> String {
    channel
        .strip_suffix(" - Topic")
        .unwrap_or(channel)
        .trim()
        .to_string()
}

fn meta_from(info: &rusty_ytdl::VideoInfo, loudness_db: Option<f64>, video_id: &str) -> Meta {
    let details = &info.video_details;
    let mut thumbnails: Vec<_> = details.thumbnails.iter().collect();
    thumbnails.sort_by_key(|thumbnail| std::cmp::Reverse(thumbnail.width));
    let art_url = thumbnails
        .first()
        .map(|thumbnail| thumbnail.url.clone())
        .or_else(|| Some(format!("https://i.ytimg.com/vi/{video_id}/hqdefault.jpg")));
    let art_small_url = thumbnails
        .iter()
        .rev()
        .find(|thumbnail| thumbnail.width >= 64)
        .or(thumbnails.last())
        .map(|thumbnail| thumbnail.url.clone())
        .or_else(|| art_url.clone());
    let channel = details
        .author
        .as_ref()
        .map(|author| author.name.as_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(&details.owner_channel_name);
    Meta {
        title: details.title.clone(),
        artist: artist_name(channel),
        artist_id: details.channel_id.clone(),
        art_url,
        art_small_url,
        duration_ms: details
            .length_seconds
            .parse::<u32>()
            .unwrap_or(0)
            .saturating_mul(1_000),
        loudness_db,
    }
}

/// The metadata the interface already has about a song fills in what
/// YouTube's video page can't say, such as the album.
fn merge_hint(track: &mut LocalTrack, hint: &LocalTrack) {
    if !hint.title.is_empty() {
        track.title.clone_from(&hint.title);
    }
    if !hint.artists.is_empty() {
        track.artists.clone_from(&hint.artists);
    }
    if !hint.album.is_empty() {
        track.album.clone_from(&hint.album);
    }
    if hint.art_url.is_some() {
        track.art_url.clone_from(&hint.art_url);
    }
    if hint.art_small_url.is_some() {
        track.art_small_url.clone_from(&hint.art_small_url);
    }
    if hint.duration_ms > 0 {
        track.duration_ms = hint.duration_ms;
    }
}

fn cache_paths(dir: &Path, video_id: &str) -> (PathBuf, PathBuf) {
    (
        dir.join(format!("{video_id}.m4a")),
        dir.join(format!("{video_id}.json")),
    )
}

/// Deletes the least recently written songs until the folder is within
/// `limit` bytes. The song just written stays.
fn prune_cache(dir: &Path, limit: u64, keep: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut songs: Vec<(std::time::SystemTime, u64, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "m4a"))
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            Some((meta.modified().ok()?, meta.len(), entry.path()))
        })
        .collect();
    let mut total: u64 = songs.iter().map(|song| song.1).sum();
    songs.sort_by_key(|song| song.0);
    for (_, size, path) in songs {
        if total <= limit {
            break;
        }
        if path.file_stem().is_some_and(|stem| stem == keep) {
            continue;
        }
        if std::fs::remove_file(&path).is_ok() {
            let _ = std::fs::remove_file(path.with_extension("json"));
            total = total.saturating_sub(size);
        }
    }
}

// ---------------------------------------------------------------------------
// The engine's shared core
// ---------------------------------------------------------------------------

/// Everything the engine, the audio thread and the fetch tasks share. It
/// outlives an engine whose connection is replaced (see [`KeptPlayer`]).
struct Core {
    cache_dir: PathBuf,
    cache_limit: Option<u64>,
    proxy: Option<reqwest::Url>,
    cookie: Option<String>,
    target_bps: u64,
    normalisation: bool,
    http: reqwest::Client,
    handle: tokio::runtime::Handle,
    state: Arc<Mutex<LocalState>>,
    notify: Mutex<Notify>,
    audio_tx: Sender<AudioCmd>,
    audio: Arc<AudioControl>,
    volume: SharedVolume,
    queue: Mutex<Queue>,
    /// Counts loads; a result that comes back for an older one is dropped.
    generation: AtomicU64,
    hints: Mutex<HashMap<String, LocalTrack>>,
    memory: Mutex<Vec<(String, Arc<Prepared>)>>,
    fetching: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl Core {
    fn start(
        config: &EngineConfig,
        proxy: Option<reqwest::Url>,
        credentials: &Credentials,
        cache_dir: PathBuf,
        notify: Notify,
    ) -> Result<Arc<Self>> {
        let mut http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(15));
        if let Some(proxy) = &proxy {
            http = http.proxy(reqwest::Proxy::all(proxy.as_str()).context("invalid proxy")?);
        }
        let http = http.build().context("unable to create the HTTP client")?;

        let state = Arc::new(Mutex::new(LocalState {
            volume: config.initial_volume,
            ..LocalState::default()
        }));
        let audio = AudioControl::new(config.buffer_ms);
        let volume = SharedVolume::at(config.initial_volume);
        let (audio_tx, audio_rx) = mpsc::channel();

        let core = Arc::new_cyclic(|weak: &Weak<Self>| {
            let worker = AudioSetup {
                rx: audio_rx,
                core: weak.clone(),
                device: config.audio_device.clone(),
                buffer_ms: config.buffer_ms,
                audio: Arc::clone(&audio),
                tap: Arc::clone(&config.tap),
                eq: Arc::clone(&config.eq),
                volume: volume.clone(),
            };
            std::thread::Builder::new()
                .name("ytm-audio".into())
                .spawn(move || worker.run())
                .expect("the audio thread starts");
            Self {
                cache_dir,
                cache_limit: config.audio_cache_limit,
                proxy,
                cookie: credentials.request_cookie(),
                target_bps: u64::from(config.bitrate_kbps) * 1_000,
                normalisation: config.normalisation,
                http,
                handle: tokio::runtime::Handle::current(),
                state,
                notify: Mutex::new(notify),
                audio_tx,
                audio,
                volume,
                queue: Mutex::new(Queue::default()),
                generation: AtomicU64::new(0),
                hints: Mutex::default(),
                memory: Mutex::default(),
                fetching: Mutex::default(),
            }
        });
        Ok(core)
    }

    fn notifier(&self) -> Notify {
        Arc::clone(&lock(&self.notify))
    }

    fn set_notifier(&self, notify: Notify) {
        *lock(&self.notify) = notify;
    }

    fn send(&self, command: AudioCmd) -> Result<()> {
        self.audio_tx
            .send(command)
            .map_err(|_| anyhow!("the audio thread has stopped"))
    }

    /// Folds `event` into the state and tells the interface if it changed.
    fn apply(&self, event: Event) {
        let snapshot = {
            let mut state = lock(&self.state);
            apply_event(&mut state, event).then(|| state.clone())
        };
        if let Some(snapshot) = snapshot {
            (self.notifier())(EngineEvent::State(snapshot));
        }
    }

    /// Changes the state and always tells the interface.
    fn update(&self, change: impl FnOnce(&mut LocalState)) {
        let snapshot = {
            let mut state = lock(&self.state);
            change(&mut state);
            state.clone()
        };
        (self.notifier())(EngineEvent::State(snapshot));
    }

    fn position_now(&self) -> u32 {
        lock(&self.state).position_now()
    }

    /// Starts loading `uri`, which replaces whatever is loading or playing.
    /// `skip_on_failure` goes on to the next song if this one will not load.
    fn start_track(
        self: &Arc<Self>,
        uri: String,
        start_ms: u32,
        play: bool,
        skip_on_failure: bool,
    ) -> Result<()> {
        let video_id = match video_id_from_uri(&uri) {
            Ok(id) => id,
            Err(error) => {
                self.audio.stopped();
                self.apply(Event::Failed(error.to_string()));
                return Err(error);
            }
        };
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.apply(Event::Loading {
            position_ms: start_ms,
        });
        self.send(AudioCmd::Halt)?;
        let core = Arc::clone(self);
        self.handle.spawn(async move {
            let outcome = core.prepare(&video_id).await;
            if core.generation.load(Ordering::SeqCst) != generation {
                return;
            }
            match outcome {
                Ok(prepared) => {
                    let sent = core.send(AudioCmd::Play {
                        generation,
                        uri,
                        prepared,
                        start_ms,
                        play,
                    });
                    if sent.is_ok() {
                        core.prefetch_next();
                    }
                }
                Err(error) => core.load_failed(&uri, &error, skip_on_failure),
            }
        });
        Ok(())
    }

    fn load_failed(self: &Arc<Self>, uri: &str, error: &LoadError, skip: bool) {
        log::warn!("unable to load {uri}: {}", error.reason());
        self.audio.stopped();
        self.apply(Event::Failed(error.message(uri)));
        if !skip {
            return;
        }
        let next = {
            let mut queue = lock(&self.queue);
            queue.failures = queue.failures.saturating_add(1);
            if queue.failures >= 5 {
                None
            } else {
                queue.next(false)
            }
        };
        if let Some(next) = next {
            let _ = self.start_track(next, 0, true, true);
        }
    }

    /// The audio thread reached the end of the song with `generation`.
    fn track_ended(self: &Arc<Self>, generation: u64) {
        if self.generation.load(Ordering::SeqCst) != generation {
            return;
        }
        let next = lock(&self.queue).next(true);
        match next {
            Some(next) => {
                let _ = self.start_track(next, 0, true, true);
            }
            None => {
                let _ = self.send(AudioCmd::Finish);
            }
        }
    }

    /// Stops what plays and what loads, letting the queued sound play out.
    fn stop_playback(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.audio.stopped();
        let _ = self.send(AudioCmd::Finish);
    }

    fn prefetch_next(self: &Arc<Self>) {
        let Some(next) = lock(&self.queue).peek_next() else {
            return;
        };
        let Ok(video_id) = video_id_from_uri(&next) else {
            return;
        };
        let core = Arc::clone(self);
        self.handle.spawn(async move {
            if let Err(error) = core.prepare(&video_id).await {
                log::debug!("prefetching {video_id} failed: {error:?}");
            }
        });
    }

    fn remembered(&self, video_id: &str) -> Option<Arc<Prepared>> {
        lock(&self.memory)
            .iter()
            .find(|(id, _)| id == video_id)
            .map(|(_, prepared)| Arc::clone(prepared))
    }

    fn remember_prepared(&self, video_id: &str, prepared: &Arc<Prepared>) {
        let mut memory = lock(&self.memory);
        memory.retain(|(id, _)| id != video_id);
        memory.push((video_id.to_string(), Arc::clone(prepared)));
        if memory.len() > MEMORY_SONGS {
            memory.remove(0);
        }
    }

    /// The song's stream and details, from memory, the cache folder, or
    /// YouTube. One download per song at a time, so a prefetch that is
    /// already under way is waited for rather than repeated.
    async fn prepare(&self, video_id: &str) -> Result<Arc<Prepared>, LoadError> {
        if let Some(prepared) = self.remembered(video_id) {
            return Ok(prepared);
        }
        let gate = Arc::clone(
            lock(&self.fetching)
                .entry(video_id.to_string())
                .or_default(),
        );
        let _turn = gate.lock().await;
        if let Some(prepared) = self.remembered(video_id) {
            return Ok(prepared);
        }
        let outcome = self.load_prepared(video_id).await;
        lock(&self.fetching).remove(video_id);
        let prepared = outcome?;
        self.remember_prepared(video_id, &prepared);
        Ok(prepared)
    }

    async fn load_prepared(&self, video_id: &str) -> Result<Arc<Prepared>, LoadError> {
        if let Some(prepared) = self.read_cached(video_id).await {
            return Ok(prepared);
        }
        let (meta, bytes) = self.download(video_id).await?;
        self.write_cached(video_id, &meta, &bytes).await;
        Ok(Arc::new(Prepared {
            meta,
            bytes: Arc::new(bytes),
        }))
    }

    async fn read_cached(&self, video_id: &str) -> Option<Arc<Prepared>> {
        let (audio, meta) = cache_paths(&self.cache_dir, video_id);
        let meta: Meta = serde_json::from_slice(&tokio::fs::read(meta).await.ok()?).ok()?;
        let bytes = tokio::fs::read(&audio).await.ok()?;
        (!bytes.is_empty()).then(|| {
            Arc::new(Prepared {
                meta,
                bytes: Arc::new(bytes),
            })
        })
    }

    async fn write_cached(&self, video_id: &str, meta: &Meta, bytes: &[u8]) {
        let (audio, meta_path) = cache_paths(&self.cache_dir, video_id);
        let partial = audio.with_extension("part");
        let written = async {
            tokio::fs::write(&partial, bytes).await?;
            tokio::fs::rename(&partial, &audio).await?;
            tokio::fs::write(&meta_path, serde_json::to_vec(meta)?).await
        }
        .await;
        match written {
            Ok(()) => {
                if let Some(limit) = self.cache_limit {
                    let dir = self.cache_dir.clone();
                    let keep = video_id.to_string();
                    let _ =
                        tokio::task::spawn_blocking(move || prune_cache(&dir, limit, &keep)).await;
                }
            }
            Err(error) => {
                log::debug!("unable to cache {video_id}: {error}");
                let _ = tokio::fs::remove_file(&partial).await;
            }
        }
    }

    async fn download(&self, video_id: &str) -> Result<(Meta, Vec<u8>), LoadError> {
        let mut request_options = rusty_ytdl::RequestOptions {
            cookies: self.cookie.clone(),
            ..Default::default()
        };
        if let Some(proxy) = &self.proxy {
            request_options.proxy = rusty_ytdl::reqwest::Proxy::all(proxy.as_str()).ok();
        }
        let video = rusty_ytdl::Video::new_with_options(
            video_id,
            rusty_ytdl::VideoOptions {
                request_options,
                ..Default::default()
            },
        )?;
        let info = tokio::time::timeout(RESOLVE_TIMEOUT, video.get_info())
            .await
            .map_err(|_| LoadError::Network("YouTube took too long to answer".into()))??;
        let format = pick_format(&info.formats, self.target_bps).ok_or_else(|| {
            LoadError::Unavailable("no audio stream this player can decode".into())
        })?;
        let meta = meta_from(&info, format.loudness_db, video_id);
        let length = format
            .content_length
            .as_deref()
            .and_then(|length| length.parse::<u64>().ok())
            .unwrap_or(0);
        let bytes = self.download_ranges(&format.url, length).await?;
        Ok((meta, bytes))
    }

    /// The stream, a range at a time as YouTube's own player asks for it:
    /// one request for the whole file is throttled.
    async fn download_ranges(&self, url: &str, length: u64) -> Result<Vec<u8>, LoadError> {
        let mut out: Vec<u8> = Vec::with_capacity(length as usize);
        loop {
            let start = out.len() as u64;
            if length > 0 && start >= length {
                break;
            }
            let end = start + DOWNLOAD_CHUNK - 1;
            let mut attempt = 0;
            let response = loop {
                let sent = self
                    .http
                    .get(url)
                    .header(reqwest::header::RANGE, format!("bytes={start}-{end}"))
                    .send()
                    .await
                    .and_then(reqwest::Response::error_for_status);
                match sent {
                    Ok(response) => break response,
                    Err(error) if attempt < 2 && error.status().is_none() => {
                        attempt += 1;
                        tokio::time::sleep(Duration::from_millis(400 * attempt)).await;
                    }
                    Err(error) => return Err(error.into()),
                }
            };
            let whole = response.status() == reqwest::StatusCode::OK;
            let body = response.bytes().await?;
            if whole {
                // The server ignored the range and sent everything.
                return Ok(body.to_vec());
            }
            if body.is_empty() {
                break;
            }
            let short = (body.len() as u64) < DOWNLOAD_CHUNK;
            out.extend_from_slice(&body);
            if length == 0 && short {
                break;
            }
        }
        if out.is_empty() {
            return Err(LoadError::Unavailable("the stream was empty".into()));
        }
        Ok(out)
    }

    fn gain_for(&self, meta: &Meta) -> f64 {
        match (self.normalisation, meta.loudness_db) {
            // YouTube plays a loud song quieter by how far it is over
            // its reference level, and never boosts a quiet one.
            (true, Some(loudness)) if loudness > 0.0 => 10f64.powf(-loudness / 20.0),
            _ => 1.0,
        }
    }
}

// ---------------------------------------------------------------------------
// The audio thread
// ---------------------------------------------------------------------------

enum AudioCmd {
    Play {
        generation: u64,
        uri: String,
        prepared: Arc<Prepared>,
        start_ms: u32,
        play: bool,
    },
    /// Drops the song being decoded, leaving what is queued to play out.
    Halt,
    Pause,
    Resume,
    Seek(u32),
    /// Plays out the queue and stops.
    Finish,
    Shutdown,
}

/// Bytes shared with the decoder, so the song is not copied for it.
struct SharedBytes(Arc<Vec<u8>>);

impl AsRef<[u8]> for SharedBytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// A song being decoded.
struct Active {
    generation: u64,
    format: Box<dyn FormatReader>,
    decoder: Box<dyn CodecDecoder>,
    track_id: u32,
    time_base: Option<TimeBase>,
    source_rate: u32,
    resampler: Option<Resampler>,
    sample_buffer: Option<SampleBuffer<f32>>,
    /// The loudness gain already applied to the samples.
    gain: f64,
    /// Where the song was when `frames` began counting.
    base_ms: u32,
    /// Frames handed to the sink since `base_ms`.
    frames: u64,
    playing: bool,
    last_report: Instant,
    duration_ms: u32,
}

/// What the audio thread is built from; the thread builds the rest.
struct AudioSetup {
    rx: Receiver<AudioCmd>,
    core: Weak<Core>,
    device: Option<String>,
    buffer_ms: u32,
    audio: Arc<AudioControl>,
    tap: Arc<AudioTap>,
    eq: crate::eq::SharedEq,
    volume: SharedVolume,
}

struct AudioWorker {
    rx: Receiver<AudioCmd>,
    core: Weak<Core>,
    sink: RodioSink,
    processing: Processing,
    audio: Arc<AudioControl>,
    active: Option<Active>,
}

impl AudioSetup {
    fn run(self) {
        let report: ErrorHook = {
            let core = self.core.clone();
            Arc::new(move |message: String| {
                if let Some(core) = core.upgrade() {
                    core.apply(Event::Error(message));
                }
            })
        };
        let saved: DeviceHook = {
            let core = self.core.clone();
            Arc::new(move |name, id| {
                if let Some(core) = core.upgrade() {
                    (core.notifier())(EngineEvent::AudioDevice {
                        name,
                        id: id.as_str().to_string(),
                    });
                }
            })
        };
        let sink = RodioSink::new(
            self.device,
            report,
            self.volume.clone(),
            self.buffer_ms,
            Arc::clone(&self.audio),
        )
        .on_device(saved);
        let processing = Processing::new(Arc::clone(&self.audio), self.tap, self.eq, self.volume);
        AudioWorker {
            rx: self.rx,
            core: self.core,
            sink,
            processing,
            audio: self.audio,
            active: None,
        }
        .run();
    }
}

/// The song's samples as interleaved stereo, whatever the layout it has.
fn to_stereo(samples: &[f32], channels: usize) -> Vec<f32> {
    match channels {
        0 => Vec::new(),
        1 => samples
            .iter()
            .flat_map(|&sample| [sample, sample])
            .collect(),
        2 => samples.to_vec(),
        n => samples
            .chunks_exact(n)
            .flat_map(|frame| [frame[0], frame[1]])
            .collect(),
    }
}

impl AudioWorker {
    fn core(&self) -> Option<Arc<Core>> {
        self.core.upgrade()
    }

    fn run(mut self) {
        loop {
            let decoding = self.active.as_ref().is_some_and(|active| active.playing);
            let command = if decoding {
                match self.rx.try_recv() {
                    Ok(command) => Some(command),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => break,
                }
            } else {
                match self.rx.recv() {
                    Ok(command) => Some(command),
                    Err(_) => break,
                }
            };
            match command {
                Some(AudioCmd::Shutdown) => break,
                Some(command) => self.handle(command),
                None => self.decode_step(),
            }
        }
        let _ = self.sink.stop();
    }

    fn handle(&mut self, command: AudioCmd) {
        match command {
            AudioCmd::Play {
                generation,
                uri,
                prepared,
                start_ms,
                play,
            } => self.play(generation, &uri, &prepared, start_ms, play),
            AudioCmd::Halt => self.active = None,
            AudioCmd::Pause => {
                let position_ms = self.position_ms();
                if let Some(active) = self.active.as_mut().filter(|active| active.playing) {
                    active.playing = false;
                    if let Some(core) = self.core() {
                        core.apply(Event::Paused { position_ms });
                    }
                    let _ = self.sink.stop();
                }
            }
            AudioCmd::Resume => {
                if self.active.as_ref().is_some_and(|active| !active.playing) {
                    let _ = self.sink.start();
                    let position_ms = self.position_ms();
                    if let Some(active) = &mut self.active {
                        active.playing = true;
                        active.last_report = Instant::now();
                    }
                    if let Some(core) = self.core() {
                        core.apply(Event::Playing { position_ms });
                    }
                }
            }
            AudioCmd::Seek(position_ms) => self.seek(position_ms),
            AudioCmd::Finish => {
                self.active = None;
                let _ = self.sink.stop();
                self.processing.stopped();
                if let Some(core) = self.core() {
                    core.apply(Event::Stopped);
                }
            }
            AudioCmd::Shutdown => {}
        }
    }

    /// Where the song is as it is heard: what has been decoded, less what
    /// is still queued for the speakers.
    fn position_ms(&self) -> u32 {
        let Some(active) = &self.active else {
            return 0;
        };
        let decoded = active.frames * 1_000 / u64::from(SAMPLE_RATE);
        let heard = (u64::from(active.base_ms) + decoded)
            .saturating_sub(u64::from(self.sink.queued_ms()))
            .max(u64::from(active.base_ms));
        heard as u32
    }

    fn play(&mut self, generation: u64, uri: &str, prepared: &Prepared, start_ms: u32, play: bool) {
        self.active = None;
        let Some(core) = self.core() else {
            return;
        };
        let mut active = match open_decoder(prepared, generation, core.gain_for(&prepared.meta)) {
            Ok(active) => active,
            Err(error) => {
                log::warn!("unable to decode {uri}: {error:#}");
                core.audio.stopped();
                core.apply(Event::Failed(format!("This item isn't available: {uri}")));
                return;
            }
        };
        if start_ms > 0 {
            match seek_active(&mut active, start_ms) {
                Ok(()) => {}
                Err(error) => log::debug!("unable to start {uri} at {start_ms} ms: {error}"),
            }
        }
        let mut track = prepared.meta.track(uri);
        if let Some(hint) = lock(&core.hints).get(&video_id_from_uri(uri).unwrap_or_default()) {
            merge_hint(&mut track, hint);
        }
        if track.duration_ms == 0 {
            track.duration_ms = active.duration_ms;
        }
        let position_ms = active.base_ms;
        active.playing = play;
        active.last_report = Instant::now();
        self.active = Some(active);
        core.audio.track_changed();
        core.apply(Event::TrackChanged(Box::new(track)));
        if play {
            let _ = self.sink.start();
            core.apply(Event::Playing { position_ms });
        } else {
            let _ = self.sink.stop();
            core.apply(Event::Paused { position_ms });
        }
        lock(&core.queue).failures = 0;
    }

    fn seek(&mut self, position_ms: u32) {
        let Some(active) = &mut self.active else {
            return;
        };
        let target = if active.duration_ms > 0 {
            position_ms.min(active.duration_ms.saturating_sub(1))
        } else {
            position_ms
        };
        if let Err(error) = seek_active(active, target) {
            log::warn!("unable to seek to {target} ms: {error}");
            return;
        }
        self.audio.seeked();
        let position_ms = active.base_ms;
        if let Some(core) = self.core() {
            core.apply(Event::Seeked { position_ms });
        }
    }

    fn decode_step(&mut self) {
        let Some(active) = &mut self.active else {
            return;
        };
        let packet = match active.format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(error))
                if error.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                return self.finished();
            }
            Err(SymphoniaError::ResetRequired) => return self.finished(),
            Err(error) => return self.broke(&error.to_string()),
        };
        if packet.track_id() != active.track_id {
            return;
        }
        let stereo = match active.decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                let capacity = decoded.capacity();
                let buffer = match &mut active.sample_buffer {
                    Some(buffer) if buffer.capacity() >= capacity * spec.channels.count() => buffer,
                    slot => slot.insert(SampleBuffer::new(capacity as u64, spec)),
                };
                buffer.copy_interleaved_ref(decoded);
                to_stereo(buffer.samples(), spec.channels.count())
            }
            // A damaged packet is skipped; the stream carries on.
            Err(SymphoniaError::DecodeError(error)) => {
                log::debug!("skipping an undecodable packet: {error}");
                return;
            }
            Err(error) => return self.broke(&error.to_string()),
        };
        let stereo = match &mut active.resampler {
            Some(resampler) => resampler.process(&stereo),
            None => stereo,
        };
        let gain = active.gain;
        let mut wide: Vec<f64> = stereo
            .iter()
            .map(|&sample| f64::from(sample) * gain)
            .collect();
        self.processing.process(&mut wide, gain);
        let out: Vec<f32> = wide.iter().map(|&sample| sample as f32).collect();
        active.frames += (out.len() / usize::from(NUM_CHANNELS)) as u64;
        if let Err(error) = self.sink.write(&out) {
            // The sink has already told the interface why.
            log::warn!("audio output failed: {error}");
            let position_ms = self.position_ms();
            if let Some(active) = &mut self.active {
                active.playing = false;
            }
            let _ = self.sink.stop();
            if let Some(core) = self.core() {
                core.apply(Event::Paused { position_ms });
            }
            return;
        }
        if let Some(active) = &mut self.active
            && active.last_report.elapsed() >= Duration::from_secs(1)
        {
            active.last_report = Instant::now();
            let position_ms = self.position_ms();
            if let Some(core) = self.core() {
                core.apply(Event::Position { position_ms });
            }
        }
    }

    /// The song ran out of packets.
    fn finished(&mut self) {
        let Some(active) = self.active.take() else {
            return;
        };
        if let Some(core) = self.core() {
            core.track_ended(active.generation);
        }
    }

    /// The stream failed part of the way through.
    fn broke(&mut self, error: &str) {
        log::warn!("playback failed: {error}");
        self.active = None;
        let _ = self.sink.stop();
        if let Some(core) = self.core() {
            core.audio.stopped();
            core.apply(Event::Failed(format!("Playback stopped: {error}")));
        }
    }
}

fn open_decoder(prepared: &Prepared, generation: u64, gain: f64) -> Result<Active> {
    let source = Cursor::new(SharedBytes(Arc::clone(&prepared.bytes)));
    let stream = MediaSourceStream::new(Box::new(source), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("m4a");
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            stream,
            &FormatOptions {
                enable_gapless: true,
                ..Default::default()
            },
            &MetadataOptions::default(),
        )
        .context("unrecognised audio format")?;
    let format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|track| track.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| anyhow!("no audio track"))?;
    let params = track.codec_params.clone();
    let decoder = symphonia::default::get_codecs()
        .make(&params, &DecoderOptions::default())
        .context("no decoder for this audio")?;
    let source_rate = params.sample_rate.unwrap_or(SAMPLE_RATE);
    let duration_ms = match (params.time_base, params.n_frames) {
        (Some(base), Some(frames)) => {
            let time = base.calc_time(frames);
            (time.seconds * 1_000 + (time.frac * 1_000.0) as u64) as u32
        }
        _ => 0,
    };
    Ok(Active {
        generation,
        track_id: track.id,
        time_base: params.time_base,
        format,
        decoder,
        source_rate,
        resampler: Resampler::new(source_rate, SAMPLE_RATE, usize::from(NUM_CHANNELS)),
        sample_buffer: None,
        gain,
        base_ms: 0,
        frames: 0,
        playing: false,
        last_report: Instant::now(),
        duration_ms,
    })
}

/// Moves the decoder to `position_ms` and starts counting from there.
fn seek_active(active: &mut Active, position_ms: u32) -> Result<()> {
    let time = Time::from(f64::from(position_ms) / 1_000.0);
    let seeked = active
        .format
        .seek(
            SeekMode::Accurate,
            SeekTo::Time {
                time,
                track_id: Some(active.track_id),
            },
        )
        .map_err(|error| anyhow!("{error}"))?;
    active.decoder.reset();
    active.resampler = Resampler::new(active.source_rate, SAMPLE_RATE, usize::from(NUM_CHANNELS));
    active.base_ms = match active.time_base {
        Some(base) => {
            let at = base.calc_time(seeked.actual_ts);
            (at.seconds * 1_000 + (at.frac * 1_000.0) as u64) as u32
        }
        None => position_ms,
    };
    active.frames = 0;
    Ok(())
}

// ---------------------------------------------------------------------------
// The engine
// ---------------------------------------------------------------------------

/// The parts of an engine that outlive it (#268). The song already
/// playing keeps playing when the engine that started it is replaced;
/// handed to the next engine, it carries on instead of stopping until the
/// new engine has loaded it again.
#[derive(Clone)]
pub struct KeptPlayer {
    core: Arc<Core>,
}

impl KeptPlayer {
    /// Stops the song, when nothing is going to pick it up.
    pub fn stop(&self) {
        self.core.stop_playback();
    }

    /// Pauses the song or plays it again, while there is no engine to ask.
    /// False when it has no song left to pause or play.
    pub fn toggle(&self) -> bool {
        let mut state = lock(&self.core.state);
        if state.track.is_none() {
            return false;
        }
        // Shown at once, and a second toggle before the audio thread
        // answers undoes the first instead of repeating it.
        match state.playback {
            Playback::Playing => {
                state.position_ms = state.position_now();
                state.position_at = None;
                state.playback = Playback::Paused;
                let _ = self.core.send(AudioCmd::Pause);
            }
            Playback::Paused => {
                state.position_at = Some(Instant::now());
                state.playback = Playback::Playing;
                let _ = self.core.send(AudioCmd::Resume);
            }
            Playback::Stopped | Playback::Loading => return false,
        }
        true
    }

    /// Whether the player still has a song, playing or paused.
    pub fn has_song(&self) -> bool {
        self.now().is_some()
    }

    /// Where the song is now and whether it plays, or `None` once the
    /// player has nothing left.
    fn now(&self) -> Option<Interrupted> {
        lock(&self.core.state).interrupted()
    }
}

pub struct Engine {
    core: Arc<Core>,
    credentials: Credentials,
    device_id: String,
    /// What was playing when the engine ended.
    interrupted: Mutex<Option<Interrupted>>,
    snapshot: Mutex<Option<LocalPlaybackSnapshot>>,
    /// The player came from an engine that was replaced and still plays
    /// what that engine left it, until something is loaded through this one.
    carrying: AtomicBool,
}

impl Engine {
    pub(crate) fn credentials(&self) -> Option<Credentials> {
        Some(self.credentials.clone())
    }

    /// Starts the player. A player `kept` from an engine that was replaced
    /// plays on, and this engine takes it over, instead of starting a player
    /// of its own. Songs are cached under `cache_dir`.
    pub async fn connect(
        config: &EngineConfig,
        proxy: Option<reqwest::Url>,
        credentials: Credentials,
        cache_dir: PathBuf,
        notify: Notify,
        kept: Option<KeptPlayer>,
    ) -> Result<Self> {
        if credentials.username.trim().is_empty() && credentials.cookie.trim().is_empty() {
            bail!("no YouTube Music account to play with");
        }
        if let Err(error) = tokio::fs::create_dir_all(&cache_dir).await {
            log::warn!("unable to create the playback cache: {error}");
        }
        let carrying = kept.is_some();
        let core = match kept {
            Some(kept) => {
                kept.core.set_notifier(Arc::clone(&notify));
                kept.core
            }
            None => Core::start(config, proxy, &credentials, cache_dir, Arc::clone(&notify))?,
        };
        core.update(|state| {
            state.connected = true;
            state.username.clone_from(&credentials.username);
        });
        Ok(Self {
            core,
            credentials,
            device_id: config.device_id(),
            interrupted: Mutex::default(),
            snapshot: Mutex::default(),
            carrying: AtomicBool::new(carrying),
        })
    }

    /// The player and what it plays through, for the engine that replaces
    /// this one. Nothing stops it: the song plays on until the next engine
    /// takes it over or [`KeptPlayer::stop`].
    pub fn keep_player(&self) -> KeptPlayer {
        KeptPlayer {
            core: Arc::clone(&self.core),
        }
    }

    /// Whether the player still plays only what an earlier engine left it:
    /// nothing has been loaded or picked up through this engine yet.
    pub fn carrying(&self) -> bool {
        self.carrying.load(Ordering::SeqCst)
    }

    fn stop_carrying(&self) {
        self.carrying.store(false, Ordering::SeqCst);
    }

    /// Playback state to resume after replacing this engine.
    pub fn interrupted(&self) -> Option<Interrupted> {
        let ended = lock(&self.interrupted).take();
        ended.or_else(|| lock(&self.core.state).interrupted())
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// The account this engine plays for.
    pub fn username(&self) -> &str {
        &self.credentials.username
    }

    /// What this engine is heard at, kept past the engine itself: the next
    /// engine starts there.
    pub(crate) fn heard(&self) -> Heard {
        Heard {
            state: Arc::clone(&self.core.state),
        }
    }

    /// Tells the engine what the interface already knows about a song, so
    /// its title, artists, album and cover are the ones the interface shows
    /// when it starts playing. YouTube's own page for the video has no album.
    pub fn remember_track(&self, track: LocalTrack) {
        if let Ok(id) = video_id_from_uri(&track.uri) {
            lock(&self.core.hints).insert(id, track);
        }
    }

    /// Applies a level set here to the output and the state at once, so
    /// the engine is heard at it from now on. The interface is told on
    /// release, not while the slider is still moving.
    fn note_volume(&self, volume: u16, preview: bool) {
        self.core.volume.set(volume);
        if preview {
            lock(&self.core.state).volume = volume;
        } else {
            self.core.update(|state| state.volume = volume);
        }
    }

    /// Whether this album is an EP. YouTube Music has no such flag.
    pub(crate) async fn album_is_ep(&self, _album_uri: &str) -> Result<bool> {
        Ok(false)
    }

    /// Lyrics as JSON; YouTube Music's come from its own API, not the engine.
    pub async fn lyrics_json(&self, _track_uri: &str) -> Result<Option<serde_json::Value>> {
        Ok(None)
    }

    /// The account's playlist tree: empty, as folders are Spotify's.
    pub async fn rootlist(&self) -> Result<Rootlist> {
        Ok(Rootlist::default())
    }

    pub fn shutdown(&self) {
        self.remember_interrupted();
        self.core.stop_playback();
        let snapshot = {
            let mut state = lock(&self.core.state);
            session_ended(&mut state, true);
            state.clone()
        };
        (self.core.notifier())(EngineEvent::State(snapshot));
        let _ = self.core.send(AudioCmd::Shutdown);
    }

    /// Ends this engine but leaves the player playing, for an engine that
    /// is not kept while the song it carries is (#268).
    pub fn close_session(&self) {
        self.remember_interrupted();
        let snapshot = {
            let mut state = lock(&self.core.state);
            session_ended(&mut state, false);
            state.clone()
        };
        (self.core.notifier())(EngineEvent::State(snapshot));
    }

    fn remember_interrupted(&self) {
        let (interrupted, position_ms) = {
            let state = lock(&self.core.state);
            (state.interrupted(), state.position_now())
        };
        if let Some(interrupted) = &interrupted {
            *lock(&self.snapshot) =
                lock(&self.core.queue).snapshot(position_ms, interrupted.playing);
        }
        *lock(&self.interrupted) = interrupted;
    }

    pub fn resume_point(&self) -> Option<PlaybackResume> {
        let interrupted = self.interrupted()?;
        let queued = lock(&self.snapshot).take().or_else(|| {
            lock(&self.core.queue).snapshot(interrupted.position_ms, interrupted.playing)
        });
        if let Some(snapshot) = queued
            && snapshot.uri == interrupted.uri
            && !snapshot.queue.is_empty()
        {
            return Some(PlaybackResume::Snapshot(
                snapshot.at_position(interrupted.position_ms, interrupted.playing),
            ));
        }
        Some(PlaybackResume::Track(LoadSpec {
            uris: vec![interrupted.uri],
            position_ms: interrupted.position_ms,
            play: interrupted.playing,
            ..LoadSpec::default()
        }))
    }

    pub fn resume(&self, resume: PlaybackResume) -> Result<()> {
        // A carried song is picked up where it has got to, playing or
        // paused as it is now, not where the engine ended: the player
        // already has it, so the restore only lines its decoder up.
        let now = self.carrying().then(|| self.keep_player().now()).flatten();
        self.stop_carrying();
        match resume_from(resume, now.as_ref()) {
            PlaybackResume::Snapshot(snapshot) => {
                lock(&self.core.queue).restore(&snapshot);
                self.core.apply(Event::Shuffle(snapshot.shuffle));
                self.core.apply(Event::Repeat(snapshot.repeat));
                self.core
                    .start_track(snapshot.uri, snapshot.position_ms, snapshot.playing, false)
            }
            PlaybackResume::Track(spec) => self.send_command(PlayerCommand::Load(spec)),
        }
    }

    pub fn command(&self, command: PlayerCommand) -> Result<()> {
        if matches!(command, PlayerCommand::Load(_) | PlayerCommand::Transfer) {
            // Something else chosen replaces the song an earlier engine left.
            self.stop_carrying();
        }
        let interrupts_audio = command_interrupts_audio(&lock(&self.core.state), &command);
        if interrupts_audio {
            self.core.audio.interrupt();
        }
        let result = self.send_command(command);
        if interrupts_audio && result.is_err() {
            self.core.audio.stopped();
        }
        result
    }

    fn send_command(&self, command: PlayerCommand) -> Result<()> {
        let core = &self.core;
        match command {
            PlayerCommand::Toggle => {
                let (playback, uri) = {
                    let state = lock(&core.state);
                    (
                        state.playback,
                        state.track.as_ref().map(|track| track.uri.clone()),
                    )
                };
                match playback {
                    Playback::Playing => core.send(AudioCmd::Pause)?,
                    Playback::Paused => core.send(AudioCmd::Resume)?,
                    Playback::Stopped => {
                        if let Some(uri) = uri {
                            core.start_track(uri, 0, true, false)?;
                        }
                    }
                    Playback::Loading => {}
                }
            }
            PlayerCommand::Next => {
                let next = lock(&core.queue).next(false);
                match next {
                    Some(uri) => core.start_track(uri, 0, true, true)?,
                    None => core.stop_playback(),
                }
            }
            PlayerCommand::Previous => {
                // Past the first seconds, Previous goes back to the start
                // of the song instead of the one before.
                let previous = if core.position_now() > 3_000 {
                    None
                } else {
                    lock(&core.queue).previous()
                };
                match previous {
                    Some(uri) => core.start_track(uri, 0, true, true)?,
                    None => {
                        core.audio.stopped();
                        core.send(AudioCmd::Seek(0))?;
                    }
                }
            }
            PlayerCommand::ClearQueue => lock(&core.queue).user.clear(),
            PlayerCommand::AddToQueue(uri) => {
                video_id_from_uri(&uri)?;
                lock(&core.queue).user.push_back(uri);
            }
            PlayerCommand::Seek(position_ms) => core.send(AudioCmd::Seek(position_ms))?,
            PlayerCommand::Volume(volume) => self.note_volume(volume, false),
            PlayerCommand::VolumePreview(volume) => self.note_volume(volume, true),
            PlayerCommand::Shuffle(enabled) => {
                lock(&core.queue).set_shuffle(enabled);
                core.apply(Event::Shuffle(enabled));
            }
            PlayerCommand::Repeat(mode) => {
                lock(&core.queue).repeat = mode;
                core.apply(Event::Repeat(mode));
            }
            PlayerCommand::Transfer => {}
            PlayerCommand::Load(spec) => self.load(spec)?,
        }
        Ok(())
    }

    fn load(&self, spec: LoadSpec) -> Result<()> {
        let core = &self.core;
        let mut uris = spec.uris;
        if uris.is_empty()
            && let Some(context) = &spec.context_uri
            && video_id_from_uri(context).is_ok()
        {
            uris.push(context.clone());
        }
        if uris.is_empty() {
            bail!("nothing to play");
        }
        let start = spec
            .offset_uri
            .as_ref()
            .and_then(|offset| uris.iter().position(|uri| uri == offset))
            .or_else(|| spec.offset_index.map(|index| index as usize))
            .unwrap_or(0)
            .min(uris.len() - 1);
        let (shuffle, repeat, first) = {
            let mut queue = lock(&core.queue);
            // A load starts the queue over: its shuffle is the one asked
            // for, and repeat stays as the user last set it.
            queue.shuffle = spec.shuffle.unwrap_or(false);
            queue.repeat = spec.repeat.unwrap_or_else(|| lock(&core.state).repeat);
            queue.user.clear();
            queue.set_context(uris, start);
            (queue.shuffle, queue.repeat, queue.current.clone())
        };
        core.apply(Event::Shuffle(shuffle));
        core.apply(Event::Repeat(repeat));
        let first = first.ok_or_else(|| anyhow!("nothing to play"))?;
        core.start_track(first, spec.position_ms, spec.play, true)
    }
}

/// What to pick up: `resume` as it was when the engine ended, or where a
/// song the player carried through the reconnect is `now`.
fn resume_from(resume: PlaybackResume, now: Option<&Interrupted>) -> PlaybackResume {
    let Some(now) = now else {
        return resume;
    };
    match resume {
        PlaybackResume::Snapshot(snapshot) => {
            if snapshot.uri == now.uri {
                PlaybackResume::Snapshot(snapshot.at_position(now.position_ms, now.playing))
            } else {
                PlaybackResume::Snapshot(snapshot)
            }
        }
        PlaybackResume::Track(mut spec) => {
            if spec.uris.first() == Some(&now.uri) {
                spec.position_ms = now.position_ms;
                spec.play = now.playing;
            }
            PlaybackResume::Track(spec)
        }
    }
}

/// What an engine is heard at, for the engine that replaces it.
#[derive(Clone)]
pub(crate) struct Heard {
    state: Arc<Mutex<LocalState>>,
}

impl Heard {
    /// The level set last, which the state holds exactly.
    pub(crate) fn level(&self) -> u16 {
        lock(&self.state).volume
    }

    /// An engine heard at `volume`, for tests that have no engine.
    #[cfg(test)]
    pub(crate) fn at(volume: u16) -> Self {
        Self {
            state: Arc::new(Mutex::new(LocalState {
                volume,
                ..LocalState::default()
            })),
        }
    }
}

// ---------------------------------------------------------------------------
// The account's playlists
// ---------------------------------------------------------------------------

/// The account's playlist tree. YouTube Music has no folders, and the
/// playlists come from its API rather than the engine, so the engine's is
/// empty.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rootlist {
    /// The rows in order, folder markers included.
    pub entries: Vec<RootlistEntry>,
    /// Playlists the account may add songs to, by URI.
    pub editable: std::collections::BTreeSet<String>,
}

/// The playlists among `entries` the account may add songs to. The engine
/// has no permission service to ask, so there are none.
pub fn editable_uris(_entries: &[RootlistEntry]) -> impl Iterator<Item = String> + '_ {
    std::iter::empty()
}

/// One row of the account's playlist tree.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RootlistEntry {
    /// A playlist, by its URI.
    Playlist(String),
    /// A folder opens; everything until its end sits inside it.
    FolderStart {
        id: String,
        name: String,
    },
    FolderEnd,
}

/// The rootlist's rows from its URIs: playlists pass through, and the
/// `start-group`/`end-group` markers older accounts bracket folders with
/// become folder rows, their names percent-decoded.
pub fn parse_rootlist(uris: &[String]) -> Vec<RootlistEntry> {
    let mut entries = Vec::new();
    let mut depth = 0usize;
    for uri in uris {
        if let Some(rest) = uri.strip_prefix("spotify:start-group:") {
            let (id, name) = match rest.split_once(':') {
                Some((id, name)) => (id.to_string(), decode_folder_name(name)),
                None => (rest.to_string(), String::new()),
            };
            entries.push(RootlistEntry::FolderStart { id, name });
            depth += 1;
        } else if uri.starts_with("spotify:end-group:") {
            if depth > 0 {
                entries.push(RootlistEntry::FolderEnd);
                depth -= 1;
            }
        } else if uri.starts_with("spotify:playlist:") || uri.starts_with("ytmusic:playlist:") {
            entries.push(RootlistEntry::Playlist(uri.clone()));
        }
    }
    // A folder that was never closed still closes here.
    entries.extend(std::iter::repeat_n(RootlistEntry::FolderEnd, depth));
    entries
}

/// Folder names arrive percent-encoded, with `+` for a space.
fn decode_folder_name(encoded: &str) -> String {
    let bytes = encoded.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            // The digits are read from the bytes this loop is already
            // walking. Taking them by slicing the text instead put the end
            // of the slice two bytes past a `%`, which is inside a character
            // whenever the next one is not ASCII: a panic rather than the
            // parse error the arm below is written for, on exactly the names
            // that arm exists for.
            b'%' if i + 2 < bytes.len() => {
                let digit = |byte: u8| (byte as char).to_digit(16);
                match (digit(bytes[i + 1]), digit(bytes[i + 2])) {
                    (Some(high), Some(low)) => {
                        out.push((high << 4 | low) as u8);
                        i += 3;
                    }
                    _ => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(uri: &str) -> LocalTrack {
        LocalTrack {
            uri: uri.into(),
            title: "Song".into(),
            duration_ms: 200_000,
            ..LocalTrack::default()
        }
    }

    const ID: &str = "dQw4w9WgXcQ";

    #[test]
    fn credentials_keep_their_secret_out_of_debug_output() {
        let credentials = Credentials::with_password("listener", "hunter2");
        assert_eq!(credentials.cookie, "hunter2");
        assert!(!format!("{credentials:?}").contains("hunter2"));
        assert_eq!(
            credentials.request_cookie(),
            None,
            "a password is not a cookie"
        );
        let cookie = Credentials::with_cookie("listener", "SID=abc; HSID=def");
        assert_eq!(
            cookie.request_cookie().as_deref(),
            Some("SID=abc; HSID=def")
        );
        let token = Credentials::with_cookie(
            crate::auth::OAUTH_ACCOUNT,
            r#"{"access_token":"a=b","refresh_token":"c"}"#,
        );
        assert_eq!(token.request_cookie(), None, "a token is not a cookie");
    }

    #[test]
    fn video_ids_come_from_every_uri_shape() {
        for uri in [
            format!("ytmusic:track:{ID}"),
            format!("ytm:{ID}"),
            format!("ytm:track:{ID}"),
            ID.to_string(),
            format!("https://music.youtube.com/watch?v={ID}&list=RDAMVM{ID}"),
            format!("https://www.youtube.com/watch?feature=share&v={ID}"),
            format!("https://youtu.be/{ID}?t=4"),
            format!("  {ID} "),
        ] {
            assert_eq!(video_id_from_uri(&uri).unwrap(), ID, "{uri}");
        }
        assert_eq!(track_uri(ID), format!("ytmusic:track:{ID}"));
    }

    #[test]
    fn other_uris_are_not_tracks() {
        for uri in [
            "spotify:track:14XWXWv5FoCbFzLksawpEe",
            "spotify:local:a:b:c:1",
            "ytmusic:playlist:PL1234567890",
            "ytm:short",
            "",
            "not a track at all",
        ] {
            assert!(video_id_from_uri(uri).is_err(), "{uri}");
        }
    }

    fn songs(count: usize) -> Vec<String> {
        (0..count)
            .map(|index| format!("ytm:song{index:07}"))
            .collect()
    }

    #[test]
    fn the_queue_plays_the_context_in_order_and_stops_at_the_end() {
        let mut queue = Queue::default();
        queue.set_context(songs(3), 1);
        assert_eq!(queue.current.as_deref(), Some("ytm:song0000001"));
        assert_eq!(queue.peek_next().as_deref(), Some("ytm:song0000002"));
        assert_eq!(queue.next(true).as_deref(), Some("ytm:song0000002"));
        assert_eq!(queue.next(true), None, "the end of the context");
        assert_eq!(queue.previous().as_deref(), Some("ytm:song0000001"));
        assert_eq!(queue.previous().as_deref(), Some("ytm:song0000000"));
        assert_eq!(queue.previous(), None);
    }

    #[test]
    fn repeat_wraps_the_context_and_repeats_a_song_only_when_it_ends() {
        let mut queue = Queue::default();
        queue.set_context(songs(2), 1);
        queue.repeat = RepeatMode::Context;
        assert_eq!(queue.next(true).as_deref(), Some("ytm:song0000000"));
        assert_eq!(queue.previous().as_deref(), Some("ytm:song0000001"));

        queue.set_context(songs(2), 0);
        queue.repeat = RepeatMode::Track;
        assert_eq!(queue.next(true).as_deref(), Some("ytm:song0000000"));
        assert_eq!(
            queue.next(false).as_deref(),
            Some("ytm:song0000001"),
            "a skip moves on even with repeat-one"
        );
    }

    #[test]
    fn manual_songs_play_before_the_context_goes_on() {
        let mut queue = Queue::default();
        queue.set_context(songs(3), 0);
        queue.user.push_back("ytm:manual00001".into());
        assert_eq!(queue.next(true).as_deref(), Some("ytm:manual00001"));
        assert_eq!(
            queue.previous().as_deref(),
            Some("ytm:song0000000"),
            "back from a manual song is the context song it interrupted"
        );
        assert_eq!(queue.next(true).as_deref(), Some("ytm:song0000001"));
    }

    #[test]
    fn shuffle_keeps_the_song_on_first_and_plays_each_once() {
        let mut queue = Queue::default();
        queue.shuffle = true;
        queue.set_context(songs(20), 7);
        assert_eq!(queue.current.as_deref(), Some("ytm:song0000007"));
        let mut seen = vec![queue.current.clone().unwrap()];
        while let Some(next) = queue.next(true) {
            seen.push(next);
        }
        seen.sort();
        assert_eq!(seen, songs(20));

        let mut queue = Queue::default();
        queue.set_context(songs(20), 7);
        queue.set_shuffle(true);
        assert_eq!(queue.context_index(), Some(7));
        queue.set_shuffle(false);
        assert_eq!((queue.pos, queue.context_index()), (7, Some(7)));
    }

    #[test]
    fn a_snapshot_restores_the_queue_where_it_was() {
        let mut queue = Queue::default();
        queue.set_context(songs(5), 2);
        queue.repeat = RepeatMode::Context;
        queue.user.push_back("ytm:manual00001".into());
        let snapshot = queue.snapshot(42_000, true).unwrap();
        assert_eq!(snapshot.uri, "ytm:song0000002");

        let mut fresh = Queue::default();
        fresh.restore(&snapshot);
        assert_eq!(fresh.current.as_deref(), Some("ytm:song0000002"));
        assert_eq!(fresh.repeat, RepeatMode::Context);
        assert_eq!(fresh.next(true).as_deref(), Some("ytm:manual00001"));
        assert_eq!(fresh.next(true).as_deref(), Some("ytm:song0000003"));
        assert_eq!(Queue::default().snapshot(0, false), None);
    }

    #[test]
    fn a_dropped_engine_keeps_the_song_and_a_shutdown_stops_it() {
        let playing = LocalState {
            connected: true,
            playback: Playback::Playing,
            track: Some(track("ytm:left")),
            position_ms: 61_000,
            position_at: Some(Instant::now()),
            ..LocalState::default()
        };
        for playback in [Playback::Playing, Playback::Paused] {
            let mut dropped = LocalState {
                playback,
                ..playing.clone()
            };
            session_ended(&mut dropped, false);
            assert!(!dropped.connected);
            assert_eq!(dropped.playback, playback);
            assert!(dropped.interrupted().is_some());
        }

        let mut shut = playing.clone();
        session_ended(&mut shut, true);
        assert_eq!(shut.playback, Playback::Stopped);
        assert_eq!(shut.position_at, None);

        let mut loading = LocalState {
            playback: Playback::Loading,
            loading: true,
            ..playing
        };
        session_ended(&mut loading, false);
        assert_eq!(loading.playback, Playback::Stopped);
        assert!(!loading.loading);
    }

    /// #268: the pickup of a song the player carried through the reconnect
    /// starts where the song has got to, as it plays now; another song's
    /// pickup is left as it was.
    #[test]
    fn a_carried_song_is_picked_up_where_it_has_got_to() {
        let spec = LoadSpec {
            uris: vec!["ytm:left".into()],
            position_ms: 42_000,
            play: true,
            ..LoadSpec::default()
        };
        let now = Interrupted {
            uri: "ytm:left".into(),
            position_ms: 47_500,
            playing: false,
        };
        let PlaybackResume::Track(picked) =
            resume_from(PlaybackResume::Track(spec.clone()), Some(&now))
        else {
            panic!("a track pickup stays one");
        };
        assert_eq!((picked.position_ms, picked.play), (47_500, false));

        let elsewhere = Interrupted {
            uri: "ytm:other".into(),
            ..now.clone()
        };
        let PlaybackResume::Track(kept) =
            resume_from(PlaybackResume::Track(spec.clone()), Some(&elsewhere))
        else {
            panic!("a track pickup stays one");
        };
        assert_eq!(kept, spec);

        let PlaybackResume::Track(untouched) =
            resume_from(PlaybackResume::Track(spec.clone()), None)
        else {
            panic!("a track pickup stays one");
        };
        assert_eq!(untouched, spec);

        let snapshot = LocalPlaybackSnapshot {
            uri: "ytm:left".into(),
            position_ms: 42_000,
            playing: true,
            queue: vec!["ytm:left".into()],
            ..LocalPlaybackSnapshot::default()
        };
        let PlaybackResume::Snapshot(moved) =
            resume_from(PlaybackResume::Snapshot(snapshot.clone()), Some(&now))
        else {
            panic!("a snapshot stays one");
        };
        assert_eq!((moved.position_ms, moved.playing), (47_500, false));
        assert_eq!(moved.queue, snapshot.queue);
    }

    #[test]
    fn each_loaded_track_has_a_new_history_sequence_but_seek_and_pause_do_not() {
        let mut state = LocalState::default();
        for sequence in [1, 2] {
            assert!(apply_event(
                &mut state,
                Event::TrackChanged(Box::new(track("ytm:one")))
            ));
            assert_eq!(state.track_sequence, sequence);
            for event in [
                Event::Playing { position_ms: 0 },
                Event::Paused {
                    position_ms: 20_000,
                },
                Event::Seeked { position_ms: 0 },
                Event::Playing { position_ms: 0 },
            ] {
                apply_event(&mut state, event);
                assert_eq!(state.track_sequence, sequence);
            }
        }
    }

    /// A track on repeat plays again with the same metadata, so only a
    /// seek tells media controls that the position went back to the start
    /// (#587). The position arrives with `Playing`, after `TrackChanged`.
    #[test]
    fn a_replay_of_the_same_track_reports_its_start_as_a_seek() {
        let mut state = LocalState::default();
        apply_event(&mut state, Event::TrackChanged(Box::new(track("ytm:one"))));
        apply_event(&mut state, Event::Playing { position_ms: 0 });
        let first_play = state.seek_sequence;
        apply_event(
            &mut state,
            Event::Position {
                position_ms: 39_900,
            },
        );

        apply_event(&mut state, Event::TrackChanged(Box::new(track("ytm:one"))));
        assert_eq!(
            state.seek_sequence, first_play,
            "the start position is not known yet"
        );
        apply_event(&mut state, Event::Playing { position_ms: 0 });
        assert_eq!(state.seek_sequence, first_play + 1);
        assert_eq!(state.position_ms, 0);

        apply_event(&mut state, Event::Paused { position_ms: 1_000 });
        assert_eq!(state.seek_sequence, first_play + 1, "reported once");
    }

    #[test]
    fn position_interpolates_only_while_playing() {
        let mut state = LocalState {
            playback: Playback::Paused,
            position_ms: 5_000,
            position_at: Some(Instant::now() - Duration::from_secs(2)),
            ..LocalState::default()
        };
        assert_eq!(state.position_now(), 5_000);
        state.playback = Playback::Playing;
        assert!(state.position_now() >= 7_000);
    }

    #[test]
    fn loading_keeps_a_playing_state_visible() {
        let mut state = LocalState {
            playback: Playback::Playing,
            ..LocalState::default()
        };
        apply_event(&mut state, Event::Loading { position_ms: 0 });
        assert_eq!(state.playback, Playback::Playing);
    }

    /// Every skip and end-of-track advance loads while the previous track
    /// still shows as playing. The load itself is carried by `loading`, so
    /// the button spinner follows the engine without flipping the transport.
    #[test]
    fn a_mid_song_load_marks_the_state_loading_until_it_starts() {
        let mut state = LocalState {
            playback: Playback::Playing,
            ..LocalState::default()
        };
        assert!(apply_event(
            &mut state,
            Event::Loading {
                position_ms: 30_000
            }
        ));
        assert_eq!(state.playback, Playback::Playing);
        assert!(state.loading);

        apply_event(&mut state, Event::Playing { position_ms: 0 });
        assert!(!state.loading);

        apply_event(&mut state, Event::Loading { position_ms: 0 });
        assert!(state.loading);
        apply_event(&mut state, Event::Stopped);
        assert!(!state.loading);

        // A load that fails has no Playing to clear the spinner, so the
        // failure must do it itself, and nothing is playing any more.
        apply_event(&mut state, Event::Playing { position_ms: 5 });
        apply_event(&mut state, Event::Loading { position_ms: 0 });
        assert!(state.loading);
        apply_event(
            &mut state,
            Event::Failed("This item isn't available".into()),
        );
        assert!(!state.loading);
        assert_eq!(state.playback, Playback::Stopped);
        assert!(state.error.is_some(), "the failure says why");
    }

    #[test]
    fn replacing_a_playing_track_interrupts_queued_audio() {
        let playing = LocalState {
            playback: Playback::Playing,
            ..LocalState::default()
        };
        let stopped = LocalState::default();
        let load = PlayerCommand::Load(LoadSpec::default());

        assert!(command_interrupts_audio(&playing, &PlayerCommand::Next));
        assert!(command_interrupts_audio(&playing, &PlayerCommand::Previous));
        assert!(command_interrupts_audio(&playing, &load));
        assert!(!command_interrupts_audio(&stopped, &PlayerCommand::Next));
        assert!(!command_interrupts_audio(
            &playing,
            &PlayerCommand::Seek(10)
        ));
    }

    #[test]
    fn a_refused_stream_has_its_own_error() {
        let message = LoadError::Refused.message("ytm:x");
        assert_eq!(message, AUDIO_KEY_REFUSED);
        let mut state = LocalState::default();
        assert!(apply_event(&mut state, Event::Failed(message)));
        assert_eq!(state.error.as_deref(), Some(AUDIO_KEY_REFUSED));
        assert!(!state.loading);
    }

    #[test]
    fn repeat_cycles_and_maps() {
        assert_eq!(RepeatMode::Off.next(), RepeatMode::Context);
        assert_eq!(RepeatMode::Track.next(), RepeatMode::Off);
        assert_eq!(RepeatMode::from_api("track"), RepeatMode::Track);
        assert_eq!(RepeatMode::Context.api_name(), "context");
    }

    fn config() -> EngineConfig {
        EngineConfig {
            buffer_ms: crate::sink::DEFAULT_BUFFER_MS,
            tap: AudioTap::new(),
            eq: crate::eq::shared(),
            device_name: "Ytmfast".into(),
            bitrate_kbps: 320,
            normalisation: false,
            autoplay: true,
            gapless: true,
            audio_device: None,
            initial_volume: 1,
            volume_dir: PathBuf::new(),
            audio_cache_dir: None,
            audio_cache_limit: None,
            proxy: crate::settings::ProxyConfig::Off,
            local_files: None,
        }
    }

    #[test]
    fn device_id_is_stable_hex() {
        let config = config();
        let id = config.device_id();
        assert_eq!(id.len(), 40);
        assert_eq!(id, config.device_id());
    }

    /// A track that was playing or paused is remembered with its position;
    /// nothing is once playback has stopped.
    #[test]
    fn an_interrupted_track_is_remembered_with_its_position() {
        let mut state = LocalState {
            track: Some(track("ytm:x")),
            playback: Playback::Playing,
            position_ms: 10_000,
            position_at: Some(Instant::now()),
            ..LocalState::default()
        };
        let resume = state.interrupted().expect("playing");
        assert_eq!(resume.uri, "ytm:x");
        assert!(resume.playing);
        assert!(resume.position_ms >= 10_000);
        state.playback = Playback::Paused;
        assert!(!state.interrupted().expect("paused").playing);
        state.playback = Playback::Stopped;
        assert!(state.interrupted().is_none());
        state.playback = Playback::Playing;
        state.track = None;
        assert!(state.interrupted().is_none());
    }

    /// The next engine starts at the level set last, exactly.
    #[test]
    fn an_engine_is_heard_at_the_level_set_last() {
        let set_here = 3276;
        assert_eq!(Heard::at(set_here).level(), set_here);
    }

    #[test]
    fn the_stream_nearest_the_bitrate_asked_for_is_picked() {
        let format = |itag: u64, bitrate: u64, container: &str, audio_only: bool| {
            let format: rusty_ytdl::VideoFormat = serde_json::from_value(serde_json::json!({
                "itag": itag,
                "mimeType": format!(
                    "{}/{container}; codecs=\"{}\"",
                    if audio_only { "audio" } else { "video" },
                    if container == "mp4" { "mp4a.40.2" } else { "opus" },
                ),
                "bitrate": bitrate,
                "url": "https://example.com/stream",
                "hasAudio": true,
                "hasVideo": !audio_only,
                "isLive": false,
                "isHLS": false,
                "isDashMPD": false,
            }))
            .unwrap();
            format
        };
        let formats = vec![
            format(139, 48_000, "mp4", true),
            format(140, 128_000, "mp4", true),
            format(141, 256_000, "mp4", true),
            format(251, 160_000, "webm", true),
            format(18, 500_000, "mp4", false),
        ];
        let picked = |kbps: u64| pick_format(&formats, kbps * 1_000).map(|format| format.itag);
        assert_eq!(picked(320), Some(141));
        assert_eq!(picked(160), Some(140));
        assert_eq!(picked(64), Some(139), "webm and video are never picked");
        assert_eq!(pick_format(&formats[3..], 320_000).map(|f| f.itag), None);
    }

    /// Needs the network and YouTube's goodwill: `cargo test -- --ignored`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn a_real_song_downloads_decodes_and_reports_its_state() {
        let seen: Arc<Mutex<Vec<LocalState>>> = Arc::default();
        let sink = Arc::clone(&seen);
        let notify: Notify = Arc::new(move |event| {
            if let EngineEvent::State(state) = event {
                sink.lock().unwrap().push(state);
            }
        });
        let dir = std::env::temp_dir().join(format!("ytmfast-live-{}", std::process::id()));
        let engine = Engine::connect(
            &config(),
            None,
            Credentials::with_cookie("listener", ""),
            dir.clone(),
            notify,
            None,
        )
        .await
        .unwrap();
        engine
            .command(PlayerCommand::Load(LoadSpec {
                uris: vec![track_uri(ID)],
                play: true,
                ..LoadSpec::default()
            }))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(90);
        while Instant::now() < deadline {
            let done = {
                let seen = seen.lock().unwrap();
                seen.iter().any(|state| state.error.is_some())
                    || seen.iter().any(|state| state.playback == Playback::Playing)
            };
            if done {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        engine.command(PlayerCommand::Seek(60_000)).unwrap();
        tokio::time::sleep(Duration::from_secs(1)).await;
        {
            let seen = seen.lock().unwrap();
            let last = seen.last().unwrap();
            assert!(last.seek_sequence >= 1, "the seek was reported");
            assert!(
                (59_000..61_000).contains(&last.position_ms),
                "{}",
                last.position_ms
            );
        }
        let seen = seen.lock().unwrap();
        for state in seen.iter() {
            eprintln!(
                "{:?} loading={} track={:?} pos={} err={:?}",
                state.playback,
                state.loading,
                state
                    .track
                    .as_ref()
                    .map(|t| (&t.title, t.artist_names(), t.duration_ms)),
                state.position_ms,
                state.error
            );
        }
        assert!(
            seen.iter().any(|state| state.track.is_some()),
            "a track was reported"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn channels_fold_into_stereo() {
        assert_eq!(to_stereo(&[1.0, 2.0], 1), [1.0, 1.0, 2.0, 2.0]);
        assert_eq!(to_stereo(&[1.0, 2.0, 3.0, 4.0], 2), [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(
            to_stereo(&[1.0, 2.0, 9.0, 9.0, 3.0, 4.0, 9.0, 9.0], 4),
            [1.0, 2.0, 3.0, 4.0]
        );
        assert!(to_stereo(&[1.0], 0).is_empty());
    }

    #[test]
    fn a_topic_channel_is_the_artist() {
        assert_eq!(artist_name("Rick Astley - Topic"), "Rick Astley");
        assert_eq!(artist_name("Rick Astley"), "Rick Astley");
    }

    #[test]
    fn what_the_interface_knows_wins_over_the_video_page() {
        let mut fetched = Meta {
            title: "Song (Official Video)".into(),
            artist: "Channel".into(),
            artist_id: "UC1".into(),
            art_url: Some("https://i.ytimg.com/a.jpg".into()),
            art_small_url: None,
            duration_ms: 0,
            loudness_db: None,
        }
        .track("ytm:x");
        let hint = LocalTrack {
            title: "Song".into(),
            album: "Album".into(),
            duration_ms: 200_000,
            ..LocalTrack::default()
        };
        merge_hint(&mut fetched, &hint);
        assert_eq!(fetched.title, "Song");
        assert_eq!(fetched.album, "Album");
        assert_eq!(fetched.duration_ms, 200_000);
        assert_eq!(
            fetched.artist_names(),
            "Channel",
            "no hinted artist keeps it"
        );
        assert_eq!(
            fetched.art_url.as_deref(),
            Some("https://i.ytimg.com/a.jpg")
        );
    }

    #[test]
    fn the_cache_is_pruned_oldest_first_and_keeps_the_new_song() {
        let dir = std::env::temp_dir().join(format!("ytmfast-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (index, id) in ["aaaaaaaaaaa", "bbbbbbbbbbb", "ccccccccccc"]
            .iter()
            .enumerate()
        {
            let (audio, meta) = cache_paths(&dir, id);
            std::fs::write(&audio, vec![0u8; 100]).unwrap();
            std::fs::write(&meta, b"{}").unwrap();
            let age = std::time::SystemTime::now() - Duration::from_secs(100 - index as u64 * 10);
            std::fs::File::options()
                .write(true)
                .open(&audio)
                .unwrap()
                .set_modified(age)
                .unwrap();
        }
        prune_cache(&dir, 150, "ccccccccccc");
        assert!(!cache_paths(&dir, "aaaaaaaaaaa").0.exists());
        assert!(!cache_paths(&dir, "aaaaaaaaaaa").1.exists());
        assert!(!cache_paths(&dir, "bbbbbbbbbbb").0.exists());
        assert!(cache_paths(&dir, "ccccccccccc").0.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_rootlist_markers_become_folders() {
        let uris: Vec<String> = [
            "spotify:playlist:aaa",
            "spotify:start-group:f1:Late%20Night+Mix",
            "spotify:playlist:bbb",
            "spotify:playlist:ccc",
            "spotify:end-group:f1",
            "ytmusic:playlist:ddd",
            "spotify:start-group:f2:Open",
            "spotify:playlist:eee",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let rows = parse_rootlist(&uris);
        assert_eq!(
            rows[0],
            RootlistEntry::Playlist("spotify:playlist:aaa".into())
        );
        assert_eq!(
            rows[1],
            RootlistEntry::FolderStart {
                id: "f1".into(),
                name: "Late Night Mix".into()
            }
        );
        assert_eq!(rows[4], RootlistEntry::FolderEnd);
        assert_eq!(
            rows[5],
            RootlistEntry::Playlist("ytmusic:playlist:ddd".into())
        );
        // The unclosed folder still closes.
        assert_eq!(rows.last(), Some(&RootlistEntry::FolderEnd));
        assert_eq!(rows.len(), 9);
        assert_eq!(editable_uris(&rows).count(), 0);
    }

    #[test]
    fn a_folder_name_with_a_bare_percent_keeps_its_percent() {
        // The decoder keeps a `%` that begins no escape and moves on, even
        // when the next character is multi-byte, where slicing the two
        // digits out of the `&str` would have panicked.
        let uris: Vec<String> = ["spotify:start-group:f1:100%25 \u{c548}\u{b155}"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            parse_rootlist(&uris)[0],
            RootlistEntry::FolderStart {
                id: "f1".into(),
                name: "100% \u{c548}\u{b155}".into()
            }
        );

        let raw: Vec<String> = ["spotify:start-group:f2:100% \u{c548}\u{b155}"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            parse_rootlist(&raw)[0],
            RootlistEntry::FolderStart {
                id: "f2".into(),
                name: "100% \u{c548}\u{b155}".into()
            }
        );
    }
}

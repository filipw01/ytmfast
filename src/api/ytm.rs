//! The YouTube Music catalogue, read through `ytmapi-rs` and mapped onto the
//! shapes in [`super::models`] that the interface already draws.
//!
//! Search, browse and public playlists work without signing in. Library
//! reads (the account's playlists, liked songs, followed artists, history)
//! need the browser cookie from music.youtube.com; without one they answer
//! with an empty page, not an error.
//!
//! Ids are YouTube Music's own: a video id for a song, a browse id for an
//! album, a channel id for an artist and a playlist id for a playlist. URIs
//! are `ytmusic:{kind}:{id}`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::StreamExt;
use tokio::sync::OnceCell;
use ytmapi_rs::auth::noauth::NoAuthToken;
use ytmapi_rs::auth::{AuthToken, BrowserToken};
use ytmapi_rs::common::{
    AlbumID, AlbumType, ArtistChannelID, Explicit, LikeStatus, PlaylistID, Thumbnail, VideoID,
    YoutubeID,
};
use ytmapi_rs::error::ErrorKind;
use ytmapi_rs::parse::{
    AlbumSong, GetAlbum, GetArtist, GetPlaylistDetails, HistoryItem, LibraryArtistSubscription,
    LibraryPlaylist, ParsedSongAlbum, ParsedSongArtist, PlaylistItem as YtPlaylistItem,
    SearchResultAlbum, SearchResultArtist, SearchResultCommunityPlaylist,
    SearchResultFeaturedPlaylist, SearchResultPlaylist, SearchResultSong, WatchPlaylistTrack,
};
use ytmapi_rs::query::GetPlaylistTracksQuery;
use ytmapi_rs::query::playlist::PrivacyStatus;
use ytmapi_rs::{Client, YtMusic, YtMusicBuilder};

use super::ApiSource;
use super::client::{ApiError, Result};
use super::models::*;

/// The id every signed-in YouTube Music account has here. The cookie carries
/// no account name the library can read, and one cookie is one account.
pub const ACCOUNT_ID: &str = "ytmusic";

/// How long a downloaded playlist, album or library list is reused, so
/// paging through it asks YouTube once.
const CACHE_TTL: Duration = Duration::from_secs(90);
const CACHE_ENTRIES: usize = 24;
/// Continuation pages followed for one playlist or library list.
const MAX_PAGES: usize = 30;
/// YouTube Music's Liked Music playlist.
const LIKED_PLAYLIST: &str = "LM";

pub fn track_uri(video_id: &str) -> String {
    crate::player::track_uri(video_id)
}

pub fn playlist_uri(id: &str) -> String {
    format!("ytmusic:playlist:{id}")
}

pub fn album_uri(id: &str) -> String {
    format!("ytmusic:album:{id}")
}

pub fn artist_uri(id: &str) -> String {
    format!("ytmusic:artist:{id}")
}

/// A playlist's id without the `VL` that browse ids put in front of it.
pub fn playlist_id(raw: &str) -> &str {
    raw.strip_prefix("VL").unwrap_or(raw)
}

/// `3:45` or `1:02:03` as milliseconds; zero for anything else.
pub fn parse_duration_ms(text: &str) -> u32 {
    let mut seconds: u64 = 0;
    for part in text.trim().split(':') {
        let Ok(part) = part.trim().parse::<u64>() else {
            return 0;
        };
        seconds = seconds.saturating_mul(60).saturating_add(part);
    }
    u32::try_from(seconds.saturating_mul(1000)).unwrap_or(u32::MAX)
}

/// The leading number of text such as `45 songs` or `1,204 tracks`.
fn leading_count(text: &str) -> u32 {
    let digits: String = text
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ',' || *c == '.')
        .filter(char::is_ascii_digit)
        .collect();
    digits.parse().unwrap_or(0)
}

fn images(thumbnails: &[Thumbnail]) -> Vec<Image> {
    thumbnails
        .iter()
        .filter(|thumbnail| !thumbnail.url.is_empty())
        .map(|thumbnail| Image {
            url: thumbnail.url.clone(),
            width: u32::try_from(thumbnail.width).ok().filter(|w| *w > 0),
            height: u32::try_from(thumbnail.height).ok().filter(|h| *h > 0),
        })
        .collect()
}

fn artist_ref(name: &str, id: Option<&str>) -> ArtistRef {
    let id = id.filter(|id| !id.is_empty());
    ArtistRef {
        id: id.map(str::to_string),
        name: name.to_string(),
        uri: id.map(artist_uri),
    }
}

fn artist_refs(artists: &[ParsedSongArtist]) -> Vec<ArtistRef> {
    artists
        .iter()
        .map(|artist| artist_ref(&artist.name, artist.id.as_ref().map(|id| id.get_raw())))
        .collect()
}

/// A page of `items` from `offset`, with `next` set while more remain.
pub fn paginate<T>(items: Vec<T>, offset: u32, limit: u32) -> Page<T> {
    let total = u32::try_from(items.len()).unwrap_or(u32::MAX);
    let start = offset.min(total) as usize;
    let size = if limit == 0 {
        items.len()
    } else {
        limit as usize
    };
    let end = start.saturating_add(size).min(items.len());
    let more = end < items.len();
    let items: Vec<T> = items.into_iter().skip(start).take(end - start).collect();
    Page {
        limit: u32::try_from(items.len()).unwrap_or(u32::MAX).max(limit),
        items,
        total,
        offset,
        next: more.then(|| "more".to_string()),
    }
}

struct TrackParts<'a> {
    video_id: &'a str,
    title: &'a str,
    artists: Vec<ArtistRef>,
    album: Option<&'a ParsedSongAlbum>,
    duration_ms: u32,
    explicit: bool,
    available: bool,
    thumbnails: &'a [Thumbnail],
    track_number: Option<u32>,
}

fn track(parts: TrackParts<'_>) -> Track {
    let cover = images(parts.thumbnails);
    let album = match parts.album {
        Some(album) => Some(Album {
            id: album.id.get_raw().to_string(),
            name: album.name.clone(),
            uri: album_uri(album.id.get_raw()),
            album_type: Some("album".into()),
            images: cover,
            artists: parts.artists.clone(),
            ..Album::default()
        }),
        None if !cover.is_empty() => Some(Album {
            images: cover,
            ..Album::default()
        }),
        None => None,
    };
    Track {
        id: Some(parts.video_id.to_string()),
        name: parts.title.to_string(),
        uri: track_uri(parts.video_id),
        duration_ms: parts.duration_ms,
        explicit: parts.explicit,
        artists: parts.artists,
        album,
        track_number: parts.track_number,
        is_playable: Some(parts.available),
        ..Track::default()
    }
}

fn is_explicit(explicit: &Explicit) -> bool {
    matches!(explicit, Explicit::IsExplicit)
}

fn song_track(song: &SearchResultSong) -> Track {
    track(TrackParts {
        video_id: song.video_id.get_raw(),
        title: &song.title,
        artists: vec![artist_ref(&song.artist, None)],
        album: song.album.as_ref(),
        duration_ms: parse_duration_ms(&song.duration),
        explicit: is_explicit(&song.explicit),
        available: true,
        thumbnails: &song.thumbnails,
        track_number: None,
    })
}

fn video_track(
    video_id: &str,
    title: &str,
    channel: &str,
    length: &str,
    thumbnails: &[Thumbnail],
) -> Track {
    track(TrackParts {
        video_id,
        title,
        artists: vec![artist_ref(channel, None)],
        album: None,
        duration_ms: parse_duration_ms(length),
        explicit: false,
        available: true,
        thumbnails,
        track_number: None,
    })
}

fn watch_track(entry: &WatchPlaylistTrack) -> Track {
    let author = entry.author.split(" • ").next().unwrap_or(&entry.author);
    video_track(
        entry.video_id.get_raw(),
        &entry.title,
        author,
        &entry.duration,
        &entry.thumbnails,
    )
}

fn album_kind(kind: &AlbumType) -> &'static str {
    match kind {
        AlbumType::Single => "single",
        AlbumType::Album | AlbumType::EP => "album",
    }
}

fn search_album(album: &SearchResultAlbum) -> Album {
    let id = album.album_id.get_raw();
    Album {
        id: id.to_string(),
        name: album.title.clone(),
        uri: album_uri(id),
        album_type: Some(album_kind(&album.album_type).into()),
        images: images(&album.thumbnails),
        artists: vec![artist_ref(&album.artist, None)],
        release_date: Some(album.year.clone()).filter(|year| !year.is_empty()),
        ..Album::default()
    }
}

fn search_artist(artist: &SearchResultArtist) -> Artist {
    let id = artist.browse_id.get_raw();
    Artist {
        id: id.to_string(),
        name: artist.artist.clone(),
        uri: artist_uri(id),
        images: images(&artist.thumbnails),
        ..Artist::default()
    }
}

fn featured_playlist(playlist: &SearchResultFeaturedPlaylist) -> Playlist {
    let id = playlist_id(playlist.playlist_id.get_raw());
    Playlist {
        id: id.to_string(),
        name: playlist.title.clone(),
        uri: playlist_uri(id),
        images: images(&playlist.thumbnails),
        owner: Owner {
            display_name: Some(playlist.author.clone()).filter(|name| !name.is_empty()),
            ..Owner::default()
        },
        public: Some(true),
        tracks: Some(TrackCount {
            total: leading_count(&playlist.songs),
        }),
        ..Playlist::default()
    }
}

fn community_playlist(playlist: &SearchResultCommunityPlaylist) -> Playlist {
    let id = playlist_id(playlist.playlist_id.get_raw());
    Playlist {
        id: id.to_string(),
        name: playlist.title.clone(),
        uri: playlist_uri(id),
        images: images(&playlist.thumbnails),
        owner: Owner {
            display_name: Some(playlist.author.clone()).filter(|name| !name.is_empty()),
            ..Owner::default()
        },
        public: Some(true),
        ..Playlist::default()
    }
}

fn library_playlist(playlist: &LibraryPlaylist) -> Playlist {
    let id = playlist_id(playlist.playlist_id.get_raw());
    Playlist {
        id: id.to_string(),
        name: playlist.title.clone(),
        uri: playlist_uri(id),
        images: images(&playlist.thumbnails),
        owner: Owner {
            id: playlist
                .author_id
                .as_ref()
                .map(|author| author.get_raw().to_string()),
            display_name: Some(playlist.author.clone()).filter(|name| !name.is_empty()),
            uri: None,
        },
        tracks: Some(TrackCount {
            total: leading_count(&playlist.tracks),
        }),
        ..Playlist::default()
    }
}

fn playlist_details(details: &GetPlaylistDetails) -> Playlist {
    let id = playlist_id(details.id.get_raw());
    Playlist {
        id: id.to_string(),
        name: details.title.clone(),
        uri: playlist_uri(id),
        description: details.description.clone().filter(|text| !text.is_empty()),
        images: images(&details.thumbnails),
        owner: Owner {
            display_name: Some(details.author.clone()).filter(|name| !name.is_empty()),
            ..Owner::default()
        },
        public: details
            .privacy
            .as_ref()
            .map(|privacy| matches!(privacy, PrivacyStatus::Public)),
        tracks: Some(TrackCount {
            total: leading_count(&details.track_count_text),
        }),
        ..Playlist::default()
    }
}

fn album_with_tracks(id: &str, details: &GetAlbum) -> Album {
    let artists = artist_refs(&details.artists);
    let images = images(&details.thumbnails);
    let mut album = Album {
        id: id.to_string(),
        name: details.title.clone(),
        uri: album_uri(id),
        album_type: Some(album_kind(&details.category).into()),
        total_tracks: u32::try_from(details.tracks.len()).ok(),
        images,
        artists,
        release_date: Some(details.year.clone()).filter(|year| !year.is_empty()),
        ..Album::default()
    };
    let tracks: Vec<Track> = details
        .tracks
        .iter()
        .map(|song| album_track(&album, song))
        .collect();
    album.tracks = Some(Page::whole(tracks));
    album
}

fn album_track(album: &Album, song: &AlbumSong) -> Track {
    Track {
        id: Some(song.video_id.get_raw().to_string()),
        name: song.title.clone(),
        uri: track_uri(song.video_id.get_raw()),
        duration_ms: parse_duration_ms(&song.duration),
        explicit: is_explicit(&song.explicit),
        artists: album.artists.clone(),
        album: Some(Album {
            tracks: None,
            ..album.clone()
        }),
        track_number: u32::try_from(song.track_no).ok(),
        is_playable: Some(true),
        ..Track::default()
    }
}

fn playlist_entry(item: &YtPlaylistItem) -> PlaylistItem {
    let track = match item {
        YtPlaylistItem::Song(song) => Some(track(TrackParts {
            video_id: song.video_id.get_raw(),
            title: &song.title,
            artists: artist_refs(&song.artists),
            album: Some(&song.album).filter(|album| !album.name.is_empty()),
            duration_ms: parse_duration_ms(&song.duration),
            explicit: is_explicit(&song.explicit),
            available: song.is_available,
            thumbnails: &song.thumbnails,
            track_number: u32::try_from(song.track_no).ok(),
        })),
        YtPlaylistItem::Video(video) => Some(track(TrackParts {
            video_id: video.video_id.get_raw(),
            title: &video.title,
            artists: vec![artist_ref(
                &video.channel_name,
                Some(video.channel_id.get_raw()),
            )],
            album: None,
            duration_ms: parse_duration_ms(&video.duration),
            explicit: false,
            available: video.is_available,
            thumbnails: &video.thumbnails,
            track_number: u32::try_from(video.track_no).ok(),
        })),
        _ => None,
    };
    PlaylistItem {
        item: track.map(PlayableItem::Track),
        ..PlaylistItem::default()
    }
}

fn history_track(item: &HistoryItem) -> Option<Track> {
    match item {
        HistoryItem::Song(song) => Some(track(TrackParts {
            video_id: song.video_id.get_raw(),
            title: &song.title,
            artists: artist_refs(&song.artists),
            album: Some(&song.album).filter(|album| !album.name.is_empty()),
            duration_ms: parse_duration_ms(&song.duration),
            explicit: is_explicit(&song.explicit),
            available: song.is_available,
            thumbnails: &song.thumbnails,
            track_number: None,
        })),
        HistoryItem::Video(video) => Some(track(TrackParts {
            video_id: video.video_id.get_raw(),
            title: &video.title,
            artists: vec![artist_ref(
                &video.channel_name,
                Some(video.channel_id.get_raw()),
            )],
            album: None,
            duration_ms: parse_duration_ms(&video.duration),
            explicit: false,
            available: video.is_available,
            thumbnails: &video.thumbnails,
            track_number: None,
        })),
        _ => None,
    }
}

fn subscription_artist(artist: &LibraryArtistSubscription) -> Artist {
    let id = artist.channel_id.get_raw();
    Artist {
        id: id.to_string(),
        name: artist.name.clone(),
        uri: artist_uri(id),
        images: images(&artist.thumbnails),
        ..Artist::default()
    }
}

fn artist_from(id: &str, artist: &GetArtist) -> Artist {
    Artist {
        id: id.to_string(),
        name: artist.name.clone(),
        uri: artist_uri(id),
        images: images(&artist.thumbnails),
        ..Artist::default()
    }
}

/// An artist's songs, as the page lists them: they carry no length.
fn artist_songs(artist: &GetArtist) -> Vec<Track> {
    artist
        .top_releases
        .songs
        .as_ref()
        .map(|songs| {
            songs
                .results
                .iter()
                .map(|song| {
                    let mut mapped = track(TrackParts {
                        video_id: song.video_id.get_raw(),
                        title: &song.title,
                        artists: artist_refs(&song.artists),
                        album: Some(&song.album).filter(|album| !album.name.is_empty()),
                        duration_ms: 0,
                        explicit: is_explicit(&song.explicit),
                        available: true,
                        thumbnails: &[],
                        track_number: None,
                    });
                    if mapped.artists.is_empty() {
                        mapped.artists = vec![artist_ref(&artist.name, None)];
                    }
                    mapped
                })
                .collect()
        })
        .unwrap_or_default()
}

fn release(
    id: &str,
    title: &str,
    year: &str,
    group: &str,
    thumbnails: &[Thumbnail],
    artist: &ArtistRef,
) -> Album {
    Album {
        id: id.to_string(),
        name: title.to_string(),
        uri: album_uri(id),
        album_type: Some(group.to_string()),
        album_group: Some(group.to_string()),
        images: images(thumbnails),
        artists: vec![artist.clone()],
        release_date: Some(year.to_string()).filter(|year| !year.is_empty()),
        ..Album::default()
    }
}

/// What a failed YouTube Music call means to the rest of the app. A call
/// made with the cookie that comes back refused or unreadable is how an
/// expired cookie shows itself.
pub fn map_error(error: ytmapi_rs::Error, signed_in: bool, source: ApiSource) -> ApiError {
    let message: String = error.to_string().chars().take(240).collect();
    match error.into_kind() {
        ErrorKind::Web { message } | ErrorKind::InvalidUserAgent(message) => {
            ApiError::Network(message)
        }
        ErrorKind::Io(error) => ApiError::Network(error.to_string()),
        ErrorKind::OtherErrorCodeInResponse {
            code: 401 | 403, ..
        } if signed_in => ApiError::SignInExpired { api_source: source },
        ErrorKind::OtherErrorCodeInResponse { code, message } => ApiError::Status {
            status: u16::try_from(code).unwrap_or(500),
            message,
        },
        ErrorKind::Header if signed_in => ApiError::SignInExpired { api_source: source },
        _ => ApiError::Decode(message),
    }
}

struct Cache<T> {
    entries: Mutex<HashMap<String, (Instant, Arc<T>)>>,
}

impl<T> Default for Cache<T> {
    fn default() -> Self {
        Self {
            entries: Mutex::default(),
        }
    }
}

impl<T> Cache<T> {
    fn get(&self, key: &str) -> Option<Arc<T>> {
        let entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        entries
            .get(key)
            .filter(|(at, _)| at.elapsed() < CACHE_TTL)
            .map(|(_, value)| Arc::clone(value))
    }

    fn put(&self, key: &str, value: T) -> Arc<T> {
        let value = Arc::new(value);
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        entries.retain(|_, (at, _)| at.elapsed() < CACHE_TTL);
        if entries.len() >= CACHE_ENTRIES
            && let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(key, _)| key.clone())
        {
            entries.remove(&oldest);
        }
        entries.insert(key.to_string(), (Instant::now(), Arc::clone(&value)));
        value
    }

    fn clear(&self) {
        self.entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
    }
}

enum Session<'a> {
    Anonymous(&'a YtMusic<NoAuthToken>),
    Cookie(&'a YtMusic<BrowserToken>),
}

/// Runs the same generic call on whichever session there is.
macro_rules! on_session {
    ($self:expr, $yt:ident => $body:expr) => {
        match $self.session().await? {
            Session::Anonymous($yt) => $body,
            Session::Cookie($yt) => $body,
        }
    };
}

/// One YouTube Music connection: unauthenticated, or with a cookie.
pub struct Ytm {
    cookie: Option<String>,
    source: ApiSource,
    anonymous: OnceCell<YtMusic<NoAuthToken>>,
    signed_in: OnceCell<YtMusic<BrowserToken>>,
    playlists: Cache<Vec<YtPlaylistItem>>,
    library: Cache<Vec<Playlist>>,
    albums: Cache<Album>,
    liked: Cache<Vec<PlaylistItem>>,
}

impl Ytm {
    pub fn new(cookie: Option<String>, source: ApiSource) -> Self {
        Self {
            cookie: cookie.filter(|cookie| !cookie.trim().is_empty()),
            source,
            anonymous: OnceCell::new(),
            signed_in: OnceCell::new(),
            playlists: Cache::default(),
            library: Cache::default(),
            albums: Cache::default(),
            liked: Cache::default(),
        }
    }

    pub fn has_cookie(&self) -> bool {
        self.cookie.is_some()
    }

    fn error(&self, error: ytmapi_rs::Error) -> ApiError {
        map_error(error, self.cookie.is_some(), self.source)
    }

    async fn session(&self) -> Result<Session<'_>> {
        match &self.cookie {
            Some(_) => self.cookie_session().await.map(Session::Cookie),
            None => {
                let yt = self
                    .anonymous
                    .get_or_try_init(|| async {
                        let client = Client::new()?;
                        YtMusicBuilder::new_with_client(client).build().await
                    })
                    .await
                    .map_err(|error| self.error(error))?;
                Ok(Session::Anonymous(yt))
            }
        }
    }

    async fn cookie_session(&self) -> Result<&YtMusic<BrowserToken>> {
        let cookie = self.cookie.clone().ok_or(ApiError::NotSignedIn)?;
        self.signed_in
            .get_or_try_init(|| async {
                let client = Client::new()?;
                YtMusicBuilder::new_with_client(client)
                    .with_browser_token_cookie(cookie)
                    .build()
                    .await
            })
            .await
            .map_err(|error| self.error(error))
    }

    /// Checks the cookie by reading the library with it. An unauthenticated
    /// connection has nothing to check.
    pub async fn verify(&self) -> Result<User> {
        if self.cookie.is_some() {
            let yt = self.cookie_session().await?;
            if let Err(error) = yt.get_library_playlists().await {
                return Err(match self.error(error) {
                    // A cookie YouTube no longer accepts gets a page the
                    // library reader cannot make sense of.
                    ApiError::Decode(_) => ApiError::SignInExpired {
                        api_source: self.source,
                    },
                    other => other,
                });
            }
        }
        Ok(User {
            id: ACCOUNT_ID.into(),
            display_name: Some("YouTube Music".into()),
            uri: Some("ytmusic:user:me".into()),
            ..User::default()
        })
    }

    // ---- search ------------------------------------------------------------

    pub async fn search(&self, query: &str, types: &[&str], limit: u32) -> Result<SearchResults> {
        let wants = |kind: &str| types.contains(&kind);
        let limit = limit.max(1) as usize;
        let mut results = SearchResults::default();
        let (songs, artists, albums, playlists) = on_session!(self, yt => {
            tokio::join!(
                async {
                    if wants("track") { Some(yt.search_songs(query).await) } else { None }
                },
                async {
                    if wants("artist") { Some(yt.search_artists(query).await) } else { None }
                },
                async {
                    if wants("album") { Some(yt.search_albums(query).await) } else { None }
                },
                async {
                    if wants("playlist") { Some(yt.search_playlists(query).await) } else { None }
                },
            )
        });
        let mut first_error = None;
        let mut answered = false;
        if let Some(found) = songs {
            match found {
                Ok(songs) => {
                    answered = true;
                    let items = songs.iter().take(limit).map(song_track).collect();
                    results.tracks = Some(Page::whole(items));
                }
                Err(error) => first_error = first_error.or(Some(error)),
            }
        }
        if let Some(found) = artists {
            match found {
                Ok(artists) => {
                    answered = true;
                    let items = artists.iter().take(limit).map(search_artist).collect();
                    results.artists = Some(Page::whole(items));
                }
                Err(error) => first_error = first_error.or(Some(error)),
            }
        }
        if let Some(found) = albums {
            match found {
                Ok(albums) => {
                    answered = true;
                    let items = albums.iter().take(limit).map(search_album).collect();
                    results.albums = Some(Page::whole(items));
                }
                Err(error) => first_error = first_error.or(Some(error)),
            }
        }
        if let Some(found) = playlists {
            match found {
                Ok(playlists) => {
                    answered = true;
                    let items = playlists
                        .iter()
                        .filter_map(|playlist| match playlist {
                            SearchResultPlaylist::Featured(featured) => {
                                Some(featured_playlist(featured))
                            }
                            SearchResultPlaylist::Community(community) => {
                                Some(community_playlist(community))
                            }
                            _ => None,
                        })
                        .take(limit)
                        .collect();
                    results.playlists = Some(Page::whole(items));
                }
                Err(error) => first_error = first_error.or(Some(error)),
            }
        }
        match first_error {
            Some(error) if !answered => Err(self.error(error)),
            Some(error) => {
                log::warn!("part of a YouTube Music search failed: {error}");
                Ok(results)
            }
            None => Ok(results),
        }
    }
}

/// Every page of a playlist, followed through its continuations.
async fn collect_playlist<A: AuthToken>(
    yt: &YtMusic<A>,
    browse_id: String,
) -> ytmapi_rs::Result<Vec<YtPlaylistItem>> {
    let query = GetPlaylistTracksQuery::new(PlaylistID::from_raw(browse_id));
    let mut pages = std::pin::pin!(yt.stream(&query));
    let mut items = Vec::new();
    let mut count = 0;
    while let Some(page) = pages.next().await {
        items.extend(page?);
        count += 1;
        if count >= MAX_PAGES {
            break;
        }
    }
    Ok(items)
}

fn browse_id(playlist: &str) -> String {
    format!("VL{}", playlist_id(playlist))
}

fn not_found(what: &str) -> ApiError {
    ApiError::Status {
        status: 404,
        message: format!("{what} isn't available on YouTube Music."),
    }
}

/// The kind and id a `ytmusic:{kind}:{id}` URI names.
pub fn parse_uri(uri: &str) -> Option<(&str, &str)> {
    let rest = uri.strip_prefix("ytmusic:")?;
    let (kind, id) = rest.split_once(':')?;
    (!id.is_empty()).then_some((kind, id))
}

impl Ytm {
    /// Without a cookie, what needs one is empty, not a failure.
    fn or_empty<T: Default>(&self, result: Result<T>) -> Result<T> {
        match result {
            Err(error) if !self.has_cookie() && !error.is_network() => Ok(T::default()),
            other => other,
        }
    }

    fn forget_library(&self) {
        self.library.clear();
        self.liked.clear();
        self.playlists.clear();
    }

    // ---- songs ---------------------------------------------------------------

    pub async fn track(&self, id: &str) -> Result<Track> {
        let entries = self.watch(id).await?;
        entries
            .iter()
            .find(|entry| entry.video_id.get_raw() == id)
            .or(entries.first())
            .map(watch_track)
            .ok_or_else(|| not_found("That song"))
    }

    /// Songs YouTube Music suggests after the first of `seeds`.
    pub async fn recommendations(&self, seeds: &[String], limit: u32) -> Result<Vec<Track>> {
        let Some(seed) = seeds
            .iter()
            .find_map(|seed| crate::player::video_id_from_uri(seed).ok())
        else {
            return Ok(Vec::new());
        };
        let entries = self.watch(&seed).await?;
        Ok(entries
            .iter()
            .filter(|entry| entry.video_id.get_raw() != seed)
            .take(limit as usize)
            .map(watch_track)
            .collect())
    }

    async fn watch(&self, video_id: &str) -> Result<Vec<WatchPlaylistTrack>> {
        let id = VideoID::from_raw(video_id.to_string());
        on_session!(self, yt => yt.get_watch_playlist_from_video_id(id).await)
            .map_err(|error| self.error(error))
    }

    // ---- albums ----------------------------------------------------------------

    pub async fn album(&self, id: &str) -> Result<Album> {
        if let Some(album) = self.albums.get(id) {
            return Ok(Album::clone(&album));
        }
        let details =
            on_session!(self, yt => yt.get_album(AlbumID::from_raw(id.to_string())).await)
                .map_err(|error| self.error(error))?;
        let album = self.albums.put(id, album_with_tracks(id, &details));
        Ok(Album::clone(&album))
    }

    pub async fn album_tracks(&self, id: &str, offset: u32, limit: u32) -> Result<Page<Track>> {
        let album = self.album(id).await?;
        let tracks = album.tracks.map(|page| page.items).unwrap_or_default();
        Ok(paginate(tracks, offset, limit))
    }

    // ---- artists -----------------------------------------------------------------

    async fn artist_page(&self, id: &str) -> Result<GetArtist> {
        let query = ArtistChannelID::from_raw(id.to_string());
        on_session!(self, yt => yt.get_artist(query).await).map_err(|error| self.error(error))
    }

    pub async fn artist(&self, id: &str) -> Result<Artist> {
        let page = self.artist_page(id).await?;
        Ok(artist_from(id, &page))
    }

    pub async fn artist_top_tracks(&self, id: &str) -> Result<Vec<Track>> {
        Ok(artist_songs(&self.artist_page(id).await?))
    }

    pub async fn related_artists(&self, id: &str) -> Result<Vec<Artist>> {
        let page = self.artist_page(id).await?;
        Ok(page
            .top_releases
            .related
            .map(|related| {
                related
                    .results
                    .iter()
                    .map(|artist| {
                        let id = artist.browse_id.get_raw();
                        Artist {
                            id: id.to_string(),
                            name: artist.title.clone(),
                            uri: artist_uri(id),
                            ..Artist::default()
                        }
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// An artist's albums and singles, the full lists when YouTube Music
    /// has them and the few its artist page shows otherwise.
    pub async fn artist_albums(
        &self,
        id: &str,
        include_groups: &str,
        offset: u32,
        limit: u32,
    ) -> Result<Page<Album>> {
        let page = self.artist_page(id).await?;
        let by = artist_ref(&page.name, Some(id));
        let wants = |group: &str| {
            include_groups
                .split(',')
                .any(|wanted| wanted.trim() == group)
        };
        let mut albums = Vec::new();
        for (group, shelf) in [
            ("album", page.top_releases.albums.as_ref()),
            ("single", page.top_releases.singles.as_ref()),
        ] {
            let Some(shelf) = shelf.filter(|_| wants(group)) else {
                continue;
            };
            let full = match (&shelf.browse_id, &shelf.params) {
                (Some(channel), Some(params)) => {
                    let (channel, params) = (channel.clone(), params.clone());
                    on_session!(self, yt => yt.get_artist_albums(channel, params).await).ok()
                }
                _ => None,
            };
            match full {
                Some(full) => albums.extend(full.iter().map(|album| {
                    release(
                        album.browse_id.get_raw(),
                        &album.title,
                        album.year.as_deref().unwrap_or_default(),
                        group,
                        &album.thumbnails,
                        &by,
                    )
                })),
                None => albums.extend(shelf.results.iter().map(|album| {
                    release(
                        album.album_id.get_raw(),
                        &album.title,
                        &album.year,
                        group,
                        &album.thumbnails,
                        &by,
                    )
                })),
            }
        }
        Ok(paginate(albums, offset, limit))
    }

    // ---- playlists ----------------------------------------------------------------

    pub async fn playlist(&self, id: &str) -> Result<Playlist> {
        let query = PlaylistID::from_raw(browse_id(id));
        let details = on_session!(self, yt => yt.get_playlist_details(query).await)
            .map_err(|error| self.error(error));
        self.or_empty(details.map(|details| playlist_details(&details)))
            .map(|mut playlist| {
                if playlist.id.is_empty() {
                    playlist.id = playlist_id(id).to_string();
                    playlist.uri = playlist_uri(&playlist.id);
                }
                playlist
            })
    }

    async fn playlist_all(&self, id: &str) -> Result<Arc<Vec<YtPlaylistItem>>> {
        let id = playlist_id(id);
        if let Some(items) = self.playlists.get(id) {
            return Ok(items);
        }
        let browse = browse_id(id);
        let items = on_session!(self, yt => collect_playlist(yt, browse).await)
            .map_err(|error| self.error(error))?;
        Ok(self.playlists.put(id, items))
    }

    pub async fn playlist_items(
        &self,
        id: &str,
        offset: u32,
        limit: u32,
    ) -> Result<Page<PlaylistItem>> {
        let items = self.playlist_all(id).await.map(|items| {
            let entries: Vec<PlaylistItem> = items.iter().map(playlist_entry).collect();
            paginate(entries, offset, limit)
        });
        self.or_empty(items)
    }

    pub async fn my_playlists(&self, offset: u32, limit: u32) -> Result<Page<Playlist>> {
        if !self.has_cookie() {
            return Ok(Page::default());
        }
        let playlists = match self.library.get("playlists") {
            Some(playlists) => playlists,
            None => {
                let listed = self
                    .cookie_session()
                    .await?
                    .get_library_playlists()
                    .await
                    .map_err(|error| self.error(error))?;
                let mapped = listed.iter().map(library_playlist).collect();
                self.library.put("playlists", mapped)
            }
        };
        Ok(paginate(Vec::clone(&playlists), offset, limit))
    }

    // ---- the account's library ---------------------------------------------------------

    async fn liked_all(&self) -> Result<Arc<Vec<PlaylistItem>>> {
        if !self.has_cookie() {
            return Ok(Arc::default());
        }
        if let Some(liked) = self.liked.get(LIKED_PLAYLIST) {
            return Ok(liked);
        }
        let items = self.playlist_all(LIKED_PLAYLIST).await?;
        let entries = items.iter().map(playlist_entry).collect();
        Ok(self.liked.put(LIKED_PLAYLIST, entries))
    }

    /// Liked Music, newest first as YouTube Music lists it.
    pub async fn saved_tracks(&self, offset: u32, limit: u32) -> Result<Page<SavedTrack>> {
        let liked = self.liked_all().await?;
        let tracks: Vec<SavedTrack> = liked
            .iter()
            .filter_map(|entry| match entry.playable() {
                Some(PlayableItem::Track(track)) => Some(SavedTrack {
                    added_at: None,
                    track: track.clone(),
                }),
                _ => None,
            })
            .collect();
        Ok(paginate(tracks, offset, limit))
    }

    pub async fn saved_albums(&self, offset: u32, limit: u32) -> Result<Page<SavedAlbum>> {
        if !self.has_cookie() {
            return Ok(Page::default());
        }
        let albums = self
            .cookie_session()
            .await?
            .get_library_albums()
            .await
            .map_err(|error| self.error(error))?;
        let albums = albums
            .iter()
            .map(|album| SavedAlbum {
                added_at: None,
                album: search_album(album),
            })
            .collect();
        Ok(paginate(albums, offset, limit))
    }

    pub async fn followed_artists(&self) -> Result<CursorPage<Artist>> {
        if !self.has_cookie() {
            return Ok(CursorPage::default());
        }
        let artists = self
            .cookie_session()
            .await?
            .get_library_artist_subscriptions()
            .await
            .map_err(|error| self.error(error))?;
        let items: Vec<Artist> = artists.iter().map(subscription_artist).collect();
        Ok(CursorPage {
            total: u32::try_from(items.len()).ok(),
            items,
            next: None,
            cursors: None,
        })
    }

    pub async fn recently_played(&self, limit: u32) -> Result<CursorPage<PlayHistory>> {
        if !self.has_cookie() {
            return Ok(CursorPage::default());
        }
        let history = self
            .cookie_session()
            .await?
            .get_history()
            .await
            .map_err(|error| self.error(error))?;
        let items = history
            .iter()
            .flat_map(|period| period.items.iter())
            .filter_map(history_track)
            .take(limit as usize)
            .map(|track| PlayHistory {
                track,
                played_at: None,
                context: None,
            })
            .collect();
        Ok(CursorPage {
            items,
            ..CursorPage::default()
        })
    }

    /// Whether each URI is liked, saved, or followed, in the order given.
    pub async fn contains(&self, uris: &[String]) -> Result<Vec<bool>> {
        if !self.has_cookie() {
            return Ok(vec![false; uris.len()]);
        }
        let mut out = Vec::with_capacity(uris.len());
        let liked = if uris
            .iter()
            .any(|uri| matches!(parse_uri(uri), Some(("track", _))))
        {
            self.liked_all().await?
        } else {
            Arc::default()
        };
        let playlists = if uris
            .iter()
            .any(|uri| matches!(parse_uri(uri), Some(("playlist", _))))
        {
            self.my_playlists(0, 0).await?.items
        } else {
            Vec::new()
        };
        for uri in uris {
            out.push(match parse_uri(uri) {
                Some(("track", _)) => liked
                    .iter()
                    .any(|entry| entry.playable().is_some_and(|item| item.uri() == uri)),
                Some(("playlist", id)) => playlists.iter().any(|list| list.id == id),
                _ => false,
            });
        }
        Ok(out)
    }

    /// Likes songs, saves playlists and albums, and follows artists.
    pub async fn set_saved(&self, uris: &[String], saved: bool) -> Result<()> {
        let yt = self.cookie_session().await?;
        let rating = if saved {
            LikeStatus::Liked
        } else {
            LikeStatus::Indifferent
        };
        for uri in uris {
            let Some((kind, id)) = parse_uri(uri) else {
                continue;
            };
            let outcome = match kind {
                "track" => {
                    yt.rate_song(VideoID::from_raw(id.to_string()), rating.clone())
                        .await
                }
                "playlist" => {
                    yt.rate_playlist(
                        PlaylistID::from_raw(playlist_id(id).to_string()),
                        rating.clone(),
                    )
                    .await
                }
                "album" => {
                    let details = yt
                        .get_album(AlbumID::from_raw(id.to_string()))
                        .await
                        .map_err(|error| self.error(error))?;
                    match details.audio_playlist_id {
                        Some(playlist) => yt.rate_playlist(playlist, rating.clone()).await,
                        None => continue,
                    }
                }
                "artist" => {
                    let channel = ArtistChannelID::from_raw(id.to_string());
                    if saved {
                        yt.subscribe_artist(channel).await
                    } else {
                        yt.unsubscribe_artists([channel]).await
                    }
                }
                _ => continue,
            };
            outcome.map_err(|error| self.error(error))?;
        }
        self.forget_library();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_name_the_kind_and_keep_the_browse_id() {
        assert_eq!(track_uri("dQw4w9WgXcQ"), "ytmusic:track:dQw4w9WgXcQ");
        assert_eq!(playlist_uri("PL1"), "ytmusic:playlist:PL1");
        assert_eq!(album_uri("MPREb_x"), "ytmusic:album:MPREb_x");
        assert_eq!(artist_uri("UC1"), "ytmusic:artist:UC1");
        assert_eq!(
            parse_uri("ytmusic:album:MPREb_x"),
            Some(("album", "MPREb_x"))
        );
        assert_eq!(parse_uri("ytmusic:album:"), None);
        assert_eq!(parse_uri("spotify:album:x"), None);
    }

    #[test]
    fn playlist_browse_ids_lose_their_prefix() {
        assert_eq!(playlist_id("VLPL123"), "PL123");
        assert_eq!(playlist_id("LM"), "LM");
    }

    #[test]
    fn durations_read_as_milliseconds() {
        assert_eq!(parse_duration_ms("3:45"), 225_000);
        assert_eq!(parse_duration_ms("1:02:03"), 3_723_000);
        assert_eq!(parse_duration_ms(""), 0);
        assert_eq!(parse_duration_ms("live"), 0);
    }

    #[test]
    fn pages_slice_a_list_and_report_the_rest() {
        let page = paginate((0..10).collect(), 0, 4);
        assert_eq!(page.items, vec![0, 1, 2, 3]);
        assert_eq!(page.total, 10);
        let page = paginate((0..10).collect(), 8, 4);
        assert_eq!(page.items, vec![8, 9]);
        let page = paginate((0..3).collect(), 9, 4);
        assert!(page.items.is_empty());
        assert_eq!(page.total, 3);
    }
}

#[cfg(test)]
mod live {
    use super::*;

    /// Talks to music.youtube.com; run with `cargo test live_smoke -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn live_smoke() {
        let ytm = Ytm::new(None, ApiSource::Personal);
        let found = ytm
            .search("daft punk", &["track", "album", "artist", "playlist"], 5)
            .await
            .unwrap();
        let tracks = found.tracks.unwrap().items;
        let albums = found.albums.unwrap().items;
        let artists = found.artists.unwrap().items;
        let playlists = found.playlists.unwrap().items;
        println!(
            "tracks={} albums={} artists={} playlists={}",
            tracks.len(),
            albums.len(),
            artists.len(),
            playlists.len()
        );
        let track = &tracks[0];
        println!("track {} {}", track.name, track.uri);
        let one = ytm.track(track.id.as_deref().unwrap()).await.unwrap();
        println!("track() -> {}", one.name);
        let album = &albums[0];
        let full = ytm.album(&album.id).await.unwrap();
        let rows = ytm.album_tracks(&album.id, 0, 50).await.unwrap();
        println!(
            "album {} {} tracks={}",
            full.name,
            full.uri,
            rows.items.len()
        );
        let artist = &artists[0];
        let top = ytm.artist_top_tracks(&artist.id).await.unwrap();
        println!(
            "artist {} top={}",
            ytm.artist(&artist.id).await.unwrap().name,
            top.len()
        );
        let albums = ytm.artist_albums(&artist.id, "album", 0, 20).await.unwrap();
        println!("artist albums={}", albums.items.len());
        let playlist = &playlists[0];
        let p = ytm.playlist(&playlist.id).await.unwrap();
        let items = ytm.playlist_items(&playlist.id, 0, 50).await.unwrap();
        println!("playlist {} items={}", p.name, items.items.len());
        let mine = ytm.my_playlists(0, 20).await.unwrap();
        println!("my playlists (no cookie) = {}", mine.items.len());
    }
}

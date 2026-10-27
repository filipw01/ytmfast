//! Catalogue reads that Spotify's streaming session used to answer.
//!
//! YouTube Music playback has no streaming session to ask, so every read in
//! this module reports that it is not available yet and the callers fall back
//! to the web client. The data types the rest of the app shares stay here.

use std::collections::HashMap;

use crate::api::ApiError;
use crate::api::models::{Album, Artist, Page, Playlist, PlaylistItem, Track};

/// Message every unavailable read fails with.
pub const UNAVAILABLE: &str = "not available on YouTube Music yet";

/// Stand-in for the streaming session YouTube Music does not have. It exists
/// so the read signatures keep a place for the session a future catalogue
/// source needs.
#[derive(Clone, Copy, Debug, Default)]
pub struct Session;

static NO_SESSION: Session = Session;

/// The placeholder every read is called with.
pub fn unavailable() -> &'static Session {
    &NO_SESSION
}

fn nothing() -> Failure {
    Failure::Retry(anyhow::anyhow!(UNAVAILABLE))
}

/// Why the session could not answer a read.
#[derive(Debug)]
pub enum Failure {
    /// The answer is final and the web client would give the same, so show it
    /// rather than ask again.
    Definitive(ApiError),
    /// The session could not be reached or understood; the web client may do
    /// better.
    Retry(anyhow::Error),
}

/// Related artist cards.
pub async fn related_artists(
    _session: &Session,
    _web: &crate::api::ApiClient,
    _id: &str,
) -> Result<Vec<Artist>, Failure> {
    Err(nothing())
}

/// An artist's top songs.
pub async fn artist_top_tracks(_session: &Session, _id: &str) -> Result<Vec<Track>, Failure> {
    Err(nothing())
}

/// A future album's own mapping and ordered preview rows.
pub async fn prerelease(
    _session: &Session,
    _web: &crate::api::ApiClient,
    _uri: &str,
) -> anyhow::Result<crate::prerelease::Prerelease> {
    anyhow::bail!(UNAVAILABLE)
}

/// The songs the account hid, as playlist and song URI pairs.
pub async fn hidden_songs(_session: &Session) -> anyhow::Result<Vec<(String, String)>> {
    anyhow::bail!(UNAVAILABLE)
}

/// Hides or unhides one song in a playlist.
pub async fn set_hidden(
    _session: &Session,
    _playlist_uri: &str,
    _track_uri: &str,
    _hidden: bool,
    _now: i64,
) -> anyhow::Result<()> {
    anyhow::bail!(UNAVAILABLE)
}

/// How many times each song of a playlist or album was played.
pub async fn play_counts(
    _session: &Session,
    _web: &crate::api::ApiClient,
    _uri: &str,
) -> anyhow::Result<HashMap<String, u64>> {
    anyhow::bail!(UNAVAILABLE)
}

/// Whether a future album is saved.
pub async fn prerelease_saved(_session: &Session, _uri: &str) -> anyhow::Result<bool> {
    anyhow::bail!(UNAVAILABLE)
}

/// Saves or unsaves a future album.
pub async fn set_prerelease_saved(
    _session: &Session,
    _http: &reqwest::Client,
    _uri: &str,
    _saved: bool,
) -> anyhow::Result<()> {
    anyhow::bail!(UNAVAILABLE)
}

/// One playlist row picked for removal: its song, its position among the
/// rows the app holds, and how many copies of the song sit before it there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemovedRow {
    pub uri: String,
    pub position: u32,
    pub copy: u32,
}

/// Removes exactly the picked rows.
pub async fn remove_playlist_rows(
    _session: &Session,
    _id: &str,
    _rows: &[RemovedRow],
) -> anyhow::Result<Option<String>> {
    anyhow::bail!(UNAVAILABLE)
}

/// A playlist's header.
pub async fn playlist(_session: &Session, _id: &str) -> Result<Playlist, Failure> {
    Err(nothing())
}

/// One page of a playlist's rows.
pub async fn items(
    _session: &Session,
    _id: &str,
    _offset: u32,
    _limit: u32,
) -> Result<Page<PlaylistItem>, Failure> {
    Err(nothing())
}

/// One page of a playlist's rows, with their added dates.
pub async fn sample(
    _session: &Session,
    _id: &str,
    _offset: u32,
    _limit: u32,
) -> Result<Page<PlaylistItem>, Failure> {
    Err(nothing())
}

/// The account's playlists.
pub async fn library(_session: &Session) -> Result<Vec<Playlist>, Failure> {
    Err(nothing())
}

/// The songs of a radio station.
pub async fn station(_session: &Session, _station: &str) -> anyhow::Result<Vec<Track>> {
    anyhow::bail!(UNAVAILABLE)
}

/// A station seeded by a song starts with that song, exactly once, followed
/// by its other songs in their order. Other kinds of station keep the order
/// they came in.
#[allow(dead_code)]
pub(crate) fn seed_first(station: &str, uris: Vec<String>) -> Vec<String> {
    let Some(seed) = track_seed(station) else {
        return uris;
    };
    let mut ordered = Vec::with_capacity(uris.len() + 1);
    ordered.push(seed.clone());
    ordered.extend(uris.into_iter().filter(|uri| *uri != seed));
    ordered
}

/// The song a station is seeded by, when the seed is a song.
#[allow(dead_code)]
fn track_seed(station: &str) -> Option<String> {
    let seed = crate::util::station_seed(station)?;
    seed.contains(":track:").then_some(seed)
}

/// Which of the given show URIs are audiobooks.
pub async fn audiobook_shows(_session: &Session, _uris: &[String]) -> anyhow::Result<Vec<String>> {
    anyhow::bail!(UNAVAILABLE)
}

/// What the profile view says about a user: their display name and avatar,
/// each when known.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserProfile {
    pub name: Option<String>,
    pub image_url: Option<String>,
}

/// The profile behind a user id; `None` when nothing answers.
pub async fn user_profile(_session: &Session, _user_id: &str) -> Option<UserProfile> {
    None
}

/// What one friend is listening to, or last played, with names to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FriendActivity {
    pub user_uri: String,
    pub user_name: String,
    pub image_url: Option<String>,
    /// Milliseconds since the epoch.
    pub timestamp_ms: i64,
    /// Whether the friend is playing the song right now.
    pub playing: bool,
    pub track_uri: String,
    pub track_name: String,
    pub artist_uri: Option<String>,
    pub artist_name: String,
    pub album_uri: Option<String>,
    pub cover_url: Option<String>,
    pub context_uri: Option<String>,
    pub context_name: Option<String>,
}

/// Names that rarely change, kept between polls.
#[derive(Default)]
pub struct FriendNames {
    #[allow(dead_code)]
    profiles: HashMap<String, UserProfile>,
    #[allow(dead_code)]
    playlists: HashMap<String, String>,
}

/// What friends are listening to, newest first.
pub async fn friend_activity(
    _session: &Session,
    _names: &tokio::sync::Mutex<FriendNames>,
) -> anyhow::Result<Vec<FriendActivity>> {
    anyhow::bail!(UNAVAILABLE)
}

/// Repair URI-derived labels of a local file, including old cached ones.
/// Compare with the raw component first so custom names and already-decoded
/// labels stay unchanged.
pub(crate) fn normalize_local_track_labels(track: &mut Track) {
    let Some(rest) = track.uri.strip_prefix("spotify:local:") else {
        return;
    };
    let parts: Vec<&str> = rest.split(':').collect();
    let [artist, album_title, track_title, _duration] = parts[..] else {
        return;
    };
    let (artist, album_title, track_title) = (
        artist.to_string(),
        album_title.to_string(),
        track_title.to_string(),
    );
    let decode = |name: &mut String, encoded: &str| {
        if name != encoded || !encoded.contains(['+', '%']) {
            return;
        }
        // Replace spaces before unescaping, so %2B stays a literal plus.
        let spaced = encoded.replace('+', " ");
        if let Ok(decoded) = percent_encoding::percent_decode_str(&spaced).decode_utf8() {
            *name = decoded.into_owned();
        }
    };
    decode(&mut track.name, &track_title);
    if let [artist_ref] = track.artists.as_mut_slice() {
        decode(&mut artist_ref.name, &artist);
    }
    if let Some(album) = &mut track.album {
        decode(&mut album.name, &album_title);
    }
}

/// Every song of `albums`, album by album.
pub async fn album_tracks(_session: &Session, _albums: &[Album]) -> Result<Vec<Track>, Failure> {
    Err(nothing())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_song_station_puts_its_seed_first_and_keeps_the_other_order() {
        let station = "spotify:station:track:4uLU6hMCjMI75M1A2tKUQC";
        let seed = "spotify:track:4uLU6hMCjMI75M1A2tKUQC".to_string();
        let uris = vec![
            "spotify:track:6rqhFgbbKwnb9MLmUQDhG6".to_string(),
            seed.clone(),
            "spotify:track:7ouMYWpwJ422jRcDASZB7P".to_string(),
        ];
        let ordered = seed_first(station, uris);
        assert_eq!(ordered.len(), 3);
        assert_eq!(ordered[0], seed);
    }

    #[test]
    fn every_read_reports_it_is_unavailable() {
        let reads = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        reads.block_on(async {
            assert!(matches!(
                playlist(unavailable(), "id").await,
                Err(Failure::Retry(_))
            ));
            assert!(station(unavailable(), "s").await.is_err());
            assert!(user_profile(unavailable(), "u").await.is_none());
        });
    }
}

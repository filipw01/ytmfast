//! The account and catalogue client, over YouTube Music.
//!
//! Every read goes through [`super::ytm`], which speaks to YouTube Music with
//! `ytmapi-rs`, and returns the shapes the interface already draws. Playback
//! belongs to the local engine, so the player commands here are accepted and
//! do nothing: there is no remote device to steer. The gateway handles
//! capability differences before dispatch.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use thiserror::Error;
use tokio::sync::Semaphore;

use super::ApiSource;
use super::models::*;
use super::ytm::{self, Ytm};
use crate::http::Http;

const MAX_IN_FLIGHT: usize = 6;
const MAX_BACKGROUND_IN_FLIGHT: usize = 4;

#[derive(Clone, Debug, Error)]
pub enum ApiError {
    #[error("not signed in")]
    NotSignedIn,
    #[error("{message}")]
    Status { status: u16, message: String },
    #[error("YouTube Music is rate limiting requests; try again in a moment")]
    RateLimited,
    #[error("YouTube Music's request quota is exhausted; try again later")]
    QuotaExhausted,
    #[error("your YouTube Music sign-in expired; please sign in again")]
    SignInExpired { api_source: ApiSource },
    #[error("network error: {0}")]
    Network(String),
    #[error("unexpected response from YouTube Music: {0}")]
    Decode(String),
}

impl ApiError {
    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Status { status, .. } => Some(*status),
            _ => None,
        }
    }

    pub fn is_network(&self) -> bool {
        matches!(self, Self::Network(_))
    }
}

impl From<reqwest::Error> for ApiError {
    fn from(error: reqwest::Error) -> Self {
        if error.is_decode() {
            Self::Decode(error.to_string())
        } else {
            Self::Network(error.to_string())
        }
    }
}

pub type Result<T> = std::result::Result<T, ApiError>;

/// Where a request's credentials come from: the YouTube Music cookie or
/// Google OAuth token.
#[derive(Clone)]
pub enum TokenProvider {
    Cookie(Arc<CookieSession>),
}

impl TokenProvider {
    fn cookie(&self) -> Result<String> {
        match self {
            Self::Cookie(session) => session.cookie(),
        }
    }
}

/// The signed-in browser cookie or Google OAuth token (as JSON), kept in
/// memory until the account it opens has been verified and only then written
/// to the protected credential store as the account's playback credential.
/// It has no `Debug`: it is a secret.
pub struct CookieSession {
    cookie: Mutex<String>,
    /// The verified account, once `remember` stored the credential: a
    /// refreshed OAuth token is stored for it from then on.
    remembered: Mutex<Option<String>>,
    lease: crate::credentials::Lease,
    storage_error: Arc<dyn Fn(crate::credentials::Error) + Send + Sync>,
    source: ApiSource,
}

impl CookieSession {
    pub fn new(
        cookie: String,
        lease: crate::credentials::Lease,
        source: ApiSource,
        storage_error: Arc<dyn Fn(crate::credentials::Error) + Send + Sync>,
    ) -> Arc<Self> {
        Arc::new(Self {
            cookie: Mutex::new(cookie),
            remembered: Mutex::new(None),
            lease,
            storage_error,
            source,
        })
    }

    fn current(&self) -> String {
        self.cookie
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// The cookie or token, unless this sign-in was replaced or signed out
    /// meanwhile.
    fn cookie(&self) -> Result<String> {
        if !self.lease.current() {
            return Err(ApiError::SignInExpired {
                api_source: self.source,
            });
        }
        Ok(self.current())
    }

    /// The credential the local engine signs in with for `account`.
    pub fn credentials(&self, account: &str) -> crate::player::Credentials {
        crate::player::Credentials::with_cookie(account, self.current())
    }

    /// Starts persistence only after the account has been verified.
    pub async fn remember(
        &self,
        account: &str,
    ) -> std::result::Result<(), crate::credentials::Error> {
        *self.remembered.lock().unwrap_or_else(|p| p.into_inner()) = Some(account.to_string());
        let pending = self.lease.save(crate::credentials::Grant::Playback(
            self.credentials(account),
        ));
        let result = pending.await;
        if let Err(error) = &result {
            (self.storage_error)(*error);
        }
        result
    }

    /// Keeps a refreshed OAuth token: in memory for the next connection, and
    /// in the credential store once the account was verified.
    pub fn refreshed(&self, token: String) {
        if !self.lease.current() || !crate::auth::is_oauth_blob(&token) {
            return;
        }
        *self.cookie.lock().unwrap_or_else(|p| p.into_inner()) = token;
        let account = self
            .remembered
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let Some(account) = account else {
            return;
        };
        let pending = self.lease.save(crate::credentials::Grant::Playback(
            self.credentials(&account),
        ));
        let storage_error = Arc::clone(&self.storage_error);
        tokio::spawn(async move {
            if let Err(error) = pending.await {
                storage_error(error);
            }
        });
    }
}

/// What to start playing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlayRequest {
    pub context_uri: Option<String>,
    pub uris: Vec<String>,
    pub offset_uri: Option<String>,
    pub offset_position: Option<u32>,
    pub position_ms: u32,
}

impl PlayRequest {
    pub fn context(uri: impl Into<String>) -> Self {
        Self {
            context_uri: Some(uri.into()),
            ..Self::default()
        }
    }

    pub fn tracks(uris: Vec<String>) -> Self {
        Self {
            uris,
            ..Self::default()
        }
    }

    pub fn starting_at_uri(mut self, uri: impl Into<String>) -> Self {
        self.offset_uri = Some(uri.into());
        self
    }

    pub fn starting_at_index(mut self, index: u32) -> Self {
        self.offset_position = Some(index);
        self
    }
}

/// Live view of the client's traffic, shared with the interface so it can
/// show that the app is talking to YouTube Music rather than being slow itself.
pub struct NetActivity {
    started_at: Instant,
    in_flight: AtomicUsize,
    /// Milliseconds since `started_at` when the oldest current burst began.
    busy_since_ms: AtomicU64,
    /// How many times the shared app was told "too many
    /// requests", so the interface can suggest a personal app.
    shared_rate_limits: AtomicU64,
}

impl Default for NetActivity {
    fn default() -> Self {
        Self {
            started_at: Instant::now(),
            in_flight: AtomicUsize::new(0),
            busy_since_ms: AtomicU64::new(0),
            shared_rate_limits: AtomicU64::new(0),
        }
    }
}

impl NetActivity {
    fn now_ms(&self) -> u64 {
        self.started_at.elapsed().as_millis() as u64
    }

    fn begin(&self) {
        if self.in_flight.fetch_add(1, Ordering::SeqCst) == 0 {
            self.busy_since_ms.store(self.now_ms(), Ordering::SeqCst);
        }
    }

    fn end(&self) {
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
    }

    /// How many "too many requests" answers the shared app has had. It only
    /// grows: compare it with an earlier reading to see a new one.
    pub fn shared_rate_limits(&self) -> u64 {
        self.shared_rate_limits.load(Ordering::SeqCst)
    }

    fn note_rate_limit(&self, source: ApiSource) {
        if source == ApiSource::Shared {
            self.shared_rate_limits.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Requests have been in flight continuously for at least `for_at_least`.
    pub fn busy(&self, for_at_least: Duration) -> bool {
        self.in_flight.load(Ordering::SeqCst) > 0
            && self
                .now_ms()
                .saturating_sub(self.busy_since_ms.load(Ordering::SeqCst))
                >= for_at_least.as_millis() as u64
    }
}

tokio::task_local! {
    static UNSEEN: ();
}

/// Runs requests nothing on screen waits for without showing them as
/// activity. Checking a grant in the background retries through rate
/// limits for as long as YouTube keeps refusing, and would otherwise hold
/// the "Waiting for YouTube Music" spinner up over an app that is ready to use.
pub async fn unseen<F: std::future::Future>(future: F) -> F::Output {
    UNSEEN.scope((), future).await
}

/// Decrements the in-flight count even if the request future is dropped.
struct ActivityGuard<'a>(&'a NetActivity);

impl<'a> ActivityGuard<'a> {
    /// Counts a request as activity, unless it runs [`unseen`].
    fn begin(activity: &'a NetActivity) -> Option<Self> {
        if UNSEEN.try_with(|_| ()).is_ok() {
            return None;
        }
        activity.begin();
        Some(Self(activity))
    }
}

impl Drop for ActivityGuard<'_> {
    fn drop(&mut self) {
        self.0.end();
    }
}

pub struct ApiClient {
    #[allow(dead_code)]
    http: Http,
    ytm: Mutex<Arc<Ytm>>,
    tokens: Mutex<Option<TokenProvider>>,
    limiter: Semaphore,
    background_limiter: Semaphore,
    cooldown_until: tokio::sync::Mutex<Instant>,
    search_limit: u32,
    artist_albums_limit: u32,
    source: ApiSource,
    activity: Arc<NetActivity>,
}

impl ApiClient {
    /// Give one authorized source independent foreground and background request budgets.
    ///
    /// `http` is kept for callers that share one transport. YouTube Music
    /// requests go through `ytmapi-rs`'s own client, which follows the
    /// proxy environment variables (`HTTPS_PROXY`, `ALL_PROXY`).
    pub fn new(
        http: impl Into<Http>,
        activity: Arc<NetActivity>,
        search_limit: u32,
        artist_albums_limit: u32,
        source: ApiSource,
    ) -> Self {
        Self {
            http: http.into(),
            ytm: Mutex::new(Arc::new(Ytm::new(None, source))),
            tokens: Mutex::new(None),
            limiter: Semaphore::new(MAX_IN_FLIGHT),
            background_limiter: Semaphore::new(MAX_BACKGROUND_IN_FLIGHT),
            cooldown_until: tokio::sync::Mutex::new(Instant::now()),
            search_limit,
            artist_albums_limit,
            source,
            activity,
        }
    }

    pub fn set_token_provider(&self, provider: Option<TokenProvider>) {
        let cookie = provider
            .as_ref()
            .and_then(|provider| provider.cookie().ok());
        let mut ytm = Ytm::new(cookie, self.source);
        if let Some(TokenProvider::Cookie(session)) = &provider {
            let session = Arc::clone(session);
            ytm = ytm.with_on_refresh(Arc::new(move |token| session.refreshed(token)));
        }
        *self.ytm.lock().unwrap_or_else(|p| p.into_inner()) = Arc::new(ytm);
        *self.tokens.lock().unwrap_or_else(|p| p.into_inner()) = provider;
    }

    /// A throttled shared app must not occupy the personal app's background slots.
    pub(crate) async fn background_permit(&self) -> Option<tokio::sync::SemaphorePermit<'_>> {
        self.background_limiter.acquire().await.ok()
    }

    /// Spotify's Home section needed a Web API token. There is none to give:
    /// the cookie must never be sent anywhere but YouTube.
    pub(crate) async fn home_context_token(&self) -> Result<String> {
        Err(ApiError::NotSignedIn)
    }

    /// A new authorization gets its own provider and request cooldown. An old
    /// request must never pick up a replacement account's credentials.
    pub fn for_authorization(&self, provider: TokenProvider) -> Self {
        let client = Self::new(
            self.http.clone(),
            self.activity.clone(),
            self.search_limit,
            self.artist_albums_limit,
            self.source,
        );
        client.set_token_provider(Some(provider));
        client
    }

    fn provider(&self) -> Result<TokenProvider> {
        self.tokens
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .ok_or(ApiError::NotSignedIn)
    }

    /// Whether requests are waiting out a cooldown.
    pub async fn cooling_down(&self) -> bool {
        *self.cooldown_until.lock().await > Instant::now()
    }

    async fn wait_for_cooldown(&self) {
        loop {
            let until = *self.cooldown_until.lock().await;
            let Some(wait) = until.checked_duration_since(Instant::now()) else {
                return;
            };
            tokio::time::sleep(wait).await;
        }
    }

    pub(super) async fn extend_cooldown(&self, wait: Duration) {
        let mut until = self.cooldown_until.lock().await;
        *until = (*until).max(Instant::now() + wait);
    }

    // ---- transport -------------------------------------------------------

    /// Runs one YouTube Music read under this client's concurrency limit,
    /// counted as activity for as long as it waits and runs.
    async fn read<T, F, Fut>(&self, what: &str, call: F) -> Result<T>
    where
        F: FnOnce(Arc<Ytm>) -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let _activity = ActivityGuard::begin(&self.activity);
        let _permit = self
            .limiter
            .acquire()
            .await
            .map_err(|_| ApiError::NotSignedIn)?;
        self.wait_for_cooldown().await;
        let ytm = Arc::clone(&self.ytm.lock().unwrap_or_else(|p| p.into_inner()));
        let started = Instant::now();
        let result = call(ytm).await;
        log::debug!(
            "YouTube Music request source={} what={what} ok={} duration_ms={}",
            self.source,
            result.is_ok(),
            started.elapsed().as_millis()
        );
        if matches!(result, Err(ApiError::RateLimited)) {
            self.activity.note_rate_limit(self.source);
            self.extend_cooldown(Duration::from_secs(5)).await;
        }
        result
    }

    fn unsupported(what: &str) -> ApiError {
        ApiError::Status {
            status: 501,
            message: format!("{what} isn't available with YouTube Music yet."),
        }
    }

    // ---- identity and player ---------------------------------------------

    /// Checks the signed-in cookie and names the account it opens.
    pub async fn me(&self) -> Result<User> {
        self.provider()?.cookie()?;
        self.read("me", |ytm| async move { ytm.verify().await })
            .await
    }

    /// The local engine is the only player: there are no remote devices.
    pub async fn devices(&self) -> Result<Vec<Device>> {
        Ok(Vec::new())
    }

    pub async fn playback_state(&self) -> Result<Option<PlaybackState>> {
        Ok(None)
    }

    pub async fn queue(&self) -> Result<Queue> {
        Ok(Queue::default())
    }

    pub async fn recently_played(
        &self,
        limit: u32,
        after: Option<&str>,
        _before: Option<&str>,
    ) -> Result<CursorPage<PlayHistory>> {
        if after.is_some() {
            return Ok(CursorPage::default());
        }
        self.read(
            "history",
            |ytm| async move { ytm.recently_played(limit).await },
        )
        .await
    }

    pub async fn play(
        &self,
        _device_id: Option<&str>,
        _request: Option<&PlayRequest>,
    ) -> Result<()> {
        Ok(())
    }

    pub async fn pause(&self, _device_id: Option<&str>) -> Result<()> {
        Ok(())
    }

    pub async fn next(&self, _device_id: Option<&str>) -> Result<()> {
        Ok(())
    }

    pub async fn previous(&self, _device_id: Option<&str>) -> Result<()> {
        Ok(())
    }

    pub async fn seek(&self, _position_ms: u32, _device_id: Option<&str>) -> Result<()> {
        Ok(())
    }

    pub async fn set_volume(&self, _percent: u8, _device_id: Option<&str>) -> Result<()> {
        Ok(())
    }

    pub async fn set_shuffle(&self, _state: bool, _device_id: Option<&str>) -> Result<()> {
        Ok(())
    }

    /// `state` is `off`, `context`, or `track`.
    pub async fn set_repeat(&self, _state: &str, _device_id: Option<&str>) -> Result<()> {
        Ok(())
    }

    pub async fn transfer(&self, _device_id: &str, _play: bool) -> Result<()> {
        Ok(())
    }

    pub async fn add_to_queue(&self, _uri: &str, _device_id: Option<&str>) -> Result<()> {
        Ok(())
    }

    pub async fn add_many_to_queue(
        &self,
        uris: &[String],
        _device_id: Option<&str>,
    ) -> (usize, Result<()>) {
        (uris.len(), Ok(()))
    }

    // ---- playlists ---------------------------------------------------------

    pub async fn my_playlists(&self, offset: u32, limit: u32) -> Result<Page<Playlist>> {
        self.read("library playlists", |ytm| async move {
            ytm.my_playlists(offset, limit).await
        })
        .await
    }

    pub async fn playlist(&self, id: &str) -> Result<Playlist> {
        self.read("playlist", |ytm| async move { ytm.playlist(id).await })
            .await
    }

    pub async fn playlist_items(
        &self,
        id: &str,
        offset: u32,
        limit: u32,
    ) -> Result<Page<PlaylistItem>> {
        self.read("playlist items", |ytm| async move {
            ytm.playlist_items(id, offset, limit).await
        })
        .await
    }

    /// Requested songs already present in a playlist.
    pub async fn playlist_duplicates(&self, id: &str, uris: &[String]) -> Result<Vec<String>> {
        let wanted: std::collections::HashSet<&str> = uris.iter().map(String::as_str).collect();
        if wanted.is_empty() {
            return Ok(Vec::new());
        }
        let mut found = std::collections::HashSet::new();
        let mut offset = 0;
        loop {
            let page = self.playlist_items(id, offset, 100).await?;
            for uri in page
                .items
                .iter()
                .filter_map(PlaylistItem::playable)
                .map(PlayableItem::uri)
            {
                if wanted.contains(uri) {
                    found.insert(uri.to_string());
                }
            }
            if found.len() == wanted.len() {
                break;
            }
            let Some(next) = page.next_offset() else {
                break;
            };
            offset = next;
        }
        Ok(uris
            .iter()
            .filter(|uri| found.contains(uri.as_str()))
            .cloned()
            .collect())
    }

    pub async fn create_playlist(
        &self,
        _name: &str,
        _public: bool,
        _description: &str,
    ) -> Result<Playlist> {
        Err(Self::unsupported("Creating playlists"))
    }

    pub async fn upload_playlist_cover(&self, _id: &str, _encoded: &str) -> Result<()> {
        Err(Self::unsupported("Changing a playlist's cover"))
    }

    pub async fn update_playlist(
        &self,
        _id: &str,
        _name: Option<&str>,
        _description: Option<&str>,
        _public: Option<bool>,
    ) -> Result<()> {
        Err(Self::unsupported("Editing playlists"))
    }

    pub async fn add_playlist_items(
        &self,
        _id: &str,
        _uris: &[String],
        _position: Option<u32>,
    ) -> Result<Option<String>> {
        Err(Self::unsupported("Adding songs to a playlist"))
    }

    pub async fn remove_playlist_items(
        &self,
        _id: &str,
        _uris: &[String],
        _snapshot_id: Option<&str>,
    ) -> Result<Option<String>> {
        Err(Self::unsupported("Removing songs from a playlist"))
    }

    pub async fn reorder_playlist(
        &self,
        _id: &str,
        _range_start: u32,
        _insert_before: u32,
        _snapshot_id: Option<&str>,
    ) -> Result<Option<String>> {
        Err(Self::unsupported("Reordering a playlist"))
    }

    pub async fn follow_playlist(&self, id: &str) -> Result<()> {
        self.save(&[ytm::playlist_uri(id)]).await
    }

    pub async fn unfollow_playlist(&self, id: &str) -> Result<()> {
        self.unsave(&[ytm::playlist_uri(id)]).await
    }

    // ---- library -----------------------------------------------------------

    /// Liked Music.
    pub async fn saved_tracks(&self, offset: u32, limit: u32) -> Result<Page<SavedTrack>> {
        self.read("liked songs", |ytm| async move {
            ytm.saved_tracks(offset, limit).await
        })
        .await
    }

    pub async fn saved_albums(&self, offset: u32, limit: u32) -> Result<Page<SavedAlbum>> {
        self.read("library albums", |ytm| async move {
            ytm.saved_albums(offset, limit).await
        })
        .await
    }

    /// The artists the account subscribes to, all in the first page.
    pub async fn followed_artists(
        &self,
        after: Option<&str>,
        _limit: u32,
    ) -> Result<CursorPage<Artist>> {
        if after.is_some() {
            return Ok(CursorPage::default());
        }
        self.read("subscriptions", |ytm| async move {
            ytm.followed_artists().await
        })
        .await
    }

    /// YouTube Music has no podcasts in this client.
    pub async fn saved_shows(&self, _offset: u32, _limit: u32) -> Result<Page<SavedShow>> {
        Ok(Page::default())
    }

    pub async fn saved_episodes(&self, _offset: u32, _limit: u32) -> Result<Page<SavedEpisode>> {
        Ok(Page::default())
    }

    /// YouTube Music keeps no listening ranking to read.
    pub async fn top_tracks(
        &self,
        _time_range: &str,
        _limit: u32,
        _offset: u32,
    ) -> Result<Page<Track>> {
        Ok(Page::default())
    }

    pub async fn top_artists(&self, _time_range: &str, _limit: u32) -> Result<Page<Artist>> {
        Ok(Page::default())
    }

    /// Likes songs, saves playlists and albums, and follows artists.
    pub async fn save(&self, uris: &[String]) -> Result<()> {
        self.read("save", |ytm| async move { ytm.set_saved(uris, true).await })
            .await
    }

    pub async fn unsave(&self, uris: &[String]) -> Result<()> {
        self.read(
            "unsave",
            |ytm| async move { ytm.set_saved(uris, false).await },
        )
        .await
    }

    /// Whether each URI is in the library, in the same order as `uris`.
    pub async fn contains(&self, uris: &[String]) -> Result<Vec<bool>> {
        self.read("contains", |ytm| async move { ytm.contains(uris).await })
            .await
    }

    // ---- catalog -----------------------------------------------------------

    pub async fn search(&self, query: &str, types: &[&str]) -> Result<SearchResults> {
        let limit = self.search_limit;
        self.read("search", |ytm| async move {
            ytm.search(query, types, limit).await
        })
        .await
    }

    pub async fn artist(&self, id: &str) -> Result<Artist> {
        self.read("artist", |ytm| async move { ytm.artist(id).await })
            .await
    }

    pub async fn artist_top_tracks(&self, id: &str) -> Result<Vec<Track>> {
        self.read("artist top tracks", |ytm| async move {
            ytm.artist_top_tracks(id).await
        })
        .await
    }

    pub async fn artist_albums(
        &self,
        id: &str,
        include_groups: &str,
        offset: u32,
        limit: u32,
    ) -> Result<Page<Album>> {
        let limit = limit.min(self.artist_albums_limit);
        self.read("artist albums", |ytm| async move {
            ytm.artist_albums(id, include_groups, offset, limit).await
        })
        .await
    }

    pub async fn related_artists(&self, id: &str) -> Result<Vec<Artist>> {
        self.read("related artists", |ytm| async move {
            ytm.related_artists(id).await
        })
        .await
    }

    pub async fn album(&self, id: &str) -> Result<Album> {
        self.read("album", |ytm| async move { ytm.album(id).await })
            .await
    }

    pub async fn album_tracks(&self, id: &str, offset: u32, limit: u32) -> Result<Page<Track>> {
        self.read("album tracks", |ytm| async move {
            ytm.album_tracks(id, offset, limit).await
        })
        .await
    }

    pub async fn show(&self, _id: &str) -> Result<Show> {
        Err(Self::unsupported("Podcasts"))
    }

    pub async fn show_episodes(
        &self,
        _id: &str,
        _offset: u32,
        _limit: u32,
    ) -> Result<Page<Episode>> {
        Ok(Page::default())
    }

    pub async fn track(&self, id: &str) -> Result<Track> {
        self.read("track", |ytm| async move { ytm.track(id).await })
            .await
    }

    pub async fn episode(&self, _id: &str) -> Result<Episode> {
        Err(Self::unsupported("Podcast episodes"))
    }

    /// Songs YouTube Music plays after the first seed song.
    pub async fn recommendations(
        &self,
        seed_tracks: &[String],
        _seed_artists: &[String],
        limit: u32,
    ) -> Result<Vec<Track>> {
        self.read("radio", |ytm| async move {
            ytm.recommendations(seed_tracks, limit).await
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(source: ApiSource) -> ApiClient {
        ApiClient::new(
            reqwest::Client::new(),
            Arc::new(NetActivity::default()),
            20,
            50,
            source,
        )
    }

    fn session(root: &str, cookie: &str) -> (crate::credentials::Store, Arc<CookieSession>) {
        let root = std::env::temp_dir().join(format!("ytmfast-{root}-{}", std::process::id()));
        let store = crate::credentials::Store::in_memory(crate::paths::AppDirs {
            config: root.join("config"),
            state: root.join("state"),
            cache: root.join("cache"),
        });
        let session = CookieSession::new(
            cookie.into(),
            store.lease(crate::credentials::Slot::Playback),
            ApiSource::Personal,
            Arc::new(|_| {}),
        );
        (store, session)
    }

    #[tokio::test]
    async fn revoked_session_cannot_return_or_persist_its_cookie() {
        let (store, session) = session("revoked-cookie", "SAPISID=dummy;");
        let slot = crate::credentials::Slot::Playback;
        // Verification can use the cookie in memory without persisting it yet.
        assert_eq!(session.cookie().unwrap(), "SAPISID=dummy;");
        assert!(store.lease(slot).load().await.unwrap().grant.is_none());
        session.remember("ytmusic").await.unwrap();
        match store.lease(slot).load().await.unwrap().grant {
            Some(crate::credentials::Grant::Playback(saved)) => {
                assert_eq!(saved.username, "ytmusic");
                assert_eq!(saved.cookie, "SAPISID=dummy;");
            }
            _ => panic!("the cookie is stored as the playback credential"),
        }
        store.revoke_spotify().unwrap();
        store.lease(slot).delete().await.unwrap();
        assert!(matches!(
            session.cookie(),
            Err(ApiError::SignInExpired { .. })
        ));
        assert_eq!(
            session.remember("ytmusic").await,
            Err(crate::credentials::Error::Stale)
        );
        assert!(store.lease(slot).load().await.unwrap().grant.is_none());
    }

    #[tokio::test]
    async fn me_needs_a_signed_in_cookie_before_any_request() {
        let client = client(ApiSource::Personal);
        assert!(matches!(client.me().await, Err(ApiError::NotSignedIn)));
    }

    #[tokio::test]
    async fn player_commands_are_accepted_and_local() {
        let client = client(ApiSource::Personal);
        client
            .play(None, Some(&PlayRequest::context("ytmusic:album:x")))
            .await
            .unwrap();
        client.pause(Some("device")).await.unwrap();
        client.set_volume(30, None).await.unwrap();
        assert!(client.devices().await.unwrap().is_empty());
        assert!(client.playback_state().await.unwrap().is_none());
        assert!(client.queue().await.unwrap().queue.is_empty());
        let uris = vec!["ytmusic:track:aaaaaaaaaaa".to_string(); 3];
        let (added, result) = client.add_many_to_queue(&uris, None).await;
        assert_eq!(added, 3);
        result.unwrap();
    }

    #[tokio::test]
    async fn what_youtube_music_cannot_do_says_so() {
        let client = client(ApiSource::Personal);
        let error = client.create_playlist("x", false, "").await.unwrap_err();
        assert_eq!(error.status(), Some(501));
        assert!(error.to_string().contains("YouTube Music"));
        assert!(client.saved_shows(0, 10).await.unwrap().items.is_empty());
        assert!(
            client
                .top_artists("long_term", 10)
                .await
                .unwrap()
                .items
                .is_empty()
        );
    }

    #[tokio::test]
    async fn the_home_token_is_never_the_cookie() {
        let client = client(ApiSource::Personal);
        let (_store, session) = session("home-token", "SAPISID=dummy;");
        client.set_token_provider(Some(TokenProvider::Cookie(session)));
        assert!(matches!(
            client.home_context_token().await,
            Err(ApiError::NotSignedIn)
        ));
    }

    #[tokio::test]
    async fn a_replaced_sign_in_stops_serving_its_cookie() {
        let (store, session) = session("replaced-cookie", "SAPISID=dummy;");
        let provider = TokenProvider::Cookie(session);
        assert!(provider.cookie().is_ok());
        store.invalidate(crate::credentials::Slot::Playback);
        assert!(matches!(
            provider.cookie(),
            Err(ApiError::SignInExpired { .. })
        ));
    }

    /// A background check of a grant is not activity the interface shows;
    /// any other request is, and stops being once it ends.
    #[tokio::test]
    async fn unseen_requests_are_not_activity() {
        let activity = NetActivity::default();
        let seen = ActivityGuard::begin(&activity);
        assert!(seen.is_some());
        assert!(activity.busy(Duration::ZERO));
        drop(seen);
        assert!(!activity.busy(Duration::ZERO));
        let hidden = unseen(async { ActivityGuard::begin(&activity).is_none() }).await;
        assert!(hidden);
        assert!(!activity.busy(Duration::ZERO));
    }

    #[tokio::test]
    async fn cooldown_state_is_owned_by_one_session() {
        let shared = client(ApiSource::Shared);
        let personal = client(ApiSource::Personal);
        shared.extend_cooldown(Duration::from_secs(10)).await;
        assert!(*shared.cooldown_until.lock().await > Instant::now());
        assert!(*personal.cooldown_until.lock().await <= Instant::now());
    }

    #[tokio::test]
    async fn shared_background_work_cannot_starve_personal_or_foreground_work() {
        let shared = client(ApiSource::Shared);
        let personal = client(ApiSource::Personal);
        let mut held = Vec::new();
        for _ in 0..MAX_BACKGROUND_IN_FLIGHT {
            held.push(shared.background_permit().await.unwrap());
        }
        assert!(shared.background_limiter.try_acquire().is_err());
        assert!(personal.background_limiter.try_acquire().is_ok());
        assert!(shared.limiter.try_acquire().is_ok());
        drop(held.pop());
        assert!(shared.background_limiter.try_acquire().is_ok());
    }
}

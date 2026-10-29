//! YouTube Music sign-in: with Google (via the system browser), or by pasting
//! a cookie.
//!
//! **Sign in with Google** opens [music.youtube.com](https://music.youtube.com)
//! in the system browser. After the listener signs in there, Ytmfast reads the
//! YouTube Music session cookie from installed browsers (Chrome, Safari,
//! Firefox, …). No OAuth client and no embedded browser engine: YouTube Music
//! rejects third-party OAuth tokens on its Innertube API.
//!
//! The advanced path is still pasting the `cookie` request header from a
//! music.youtube.com tab's developer tools.
//!
//! Cookies (and legacy OAuth JSON blobs) are bearer secrets. They are sent
//! only to Google and YouTube, kept only in the protected credential store as
//! [`crate::player::Credentials::with_cookie`], and never logged.

use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use ytmapi_rs::auth::{OAuthToken, OAuthTokenGenerator};
use ytmapi_rs::error::ErrorKind;

/// The account name an OAuth token is stored under. The token carries no
/// account name the library can read.
pub const OAUTH_ACCOUNT: &str = "ytmusic-oauth";

/// Where Google explains creating the OAuth client.
pub const OAUTH_SETUP_URL: &str =
    "https://developers.google.com/youtube/registering_an_application";

/// Where the OAuth client is created.
pub const OAUTH_CONSOLE_URL: &str = "https://console.cloud.google.com/apis/credentials";

/// The listener's own Google Cloud OAuth client, of the type "TVs and
/// Limited Input devices". Deliberately has no Debug implementation.
#[derive(Clone, PartialEq, Eq)]
pub struct OAuthClient {
    pub id: String,
    pub secret: String,
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn oauth_client_from(
    settings_id: Option<&str>,
    settings_secret: Option<&str>,
    env: impl Fn(&str) -> Option<String>,
) -> Option<OAuthClient> {
    let from_env = |names: [&str; 2]| {
        names
            .into_iter()
            .find_map(|name| non_empty(env(name).as_deref()))
    };
    let id = non_empty(settings_id)
        .or_else(|| from_env(["YTMFAST_OAUTH_CLIENT_ID", "YOUTUI_OAUTH_CLIENT_ID"]))?;
    let secret = non_empty(settings_secret)
        .or_else(|| from_env(["YTMFAST_OAUTH_CLIENT_SECRET", "YOUTUI_OAUTH_CLIENT_SECRET"]))?;
    Some(OAuthClient { id, secret })
}

/// The OAuth client from Settings, field by field falling back to
/// `YTMFAST_OAUTH_CLIENT_ID` / `YTMFAST_OAUTH_CLIENT_SECRET` and then
/// youtui's `YOUTUI_OAUTH_*` names.
pub fn oauth_credentials_from_env_or_settings(
    settings_id: Option<&str>,
    settings_secret: Option<&str>,
) -> Option<OAuthClient> {
    oauth_client_from(settings_id, settings_secret, |name| {
        std::env::var(name).ok()
    })
}

/// Whether a stored credential is an OAuth token rather than a cookie.
pub fn is_oauth_blob(text: &str) -> bool {
    text.trim_start().starts_with('{')
}

pub fn encode_oauth_token(token: &OAuthToken) -> String {
    serde_json::to_string(token).unwrap_or_default()
}

pub fn decode_oauth_token(text: &str) -> Option<OAuthToken> {
    if !is_oauth_blob(text) {
        return None;
    }
    serde_json::from_str(text.trim()).ok()
}

/// When the token's access stops working, in Unix seconds. `ytmapi-rs`
/// keeps the fields private, so they are read back from its own JSON.
pub fn oauth_expires_at(token: &OAuthToken) -> Option<u64> {
    let value = serde_json::to_value(token).ok()?;
    let issued = value.pointer("/request_time/secs_since_epoch")?.as_u64()?;
    let lasts = value.get("expires_in")?.as_u64()?;
    Some(issued.saturating_add(lasts))
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// What a stored playback credential signs in with: a cookie in the form
/// `ytmapi-rs` expects, or an OAuth token as stored. `None` for anything
/// else, such as a grant from the Spotify days.
pub fn stored_sign_in(text: &str) -> Option<String> {
    if is_oauth_blob(text) {
        return decode_oauth_token(text).map(|_| text.trim().to_string());
    }
    normalize_cookie(text).ok()
}

/// Why Google sign-in did not finish.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OAuthProblem {
    /// The sign-in was cancelled here.
    Cancelled,
    /// The listener said no on Google's page.
    Denied,
    /// The code ran out before it was confirmed.
    Expired,
    /// Google does not know the client, or it is not a "TVs and Limited
    /// Input devices" client.
    InvalidClient,
    Network(String),
    /// Anything else Google answered, as its error code.
    Other(String),
}

impl std::fmt::Display for OAuthProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => write!(f, "Google sign-in was cancelled"),
            Self::Denied => write!(f, "Google sign-in was declined"),
            Self::Expired => write!(f, "the Google sign-in code expired"),
            Self::InvalidClient => write!(f, "Google rejected the OAuth client"),
            Self::Network(message) => write!(f, "network error: {message}"),
            Self::Other(code) => write!(f, "Google answered {code}"),
        }
    }
}

/// The `error` code in a Google OAuth error answer.
fn google_error(text: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    value.get("error")?.as_str().map(str::to_string)
}

/// What a Google error answer means while waiting for the listener.
#[derive(Debug, PartialEq, Eq)]
enum Poll {
    Pending,
    SlowDown,
    Failed(OAuthProblem),
}

fn classify(code: &str) -> Poll {
    match code {
        "authorization_pending" => Poll::Pending,
        "slow_down" => Poll::SlowDown,
        "access_denied" => Poll::Failed(OAuthProblem::Denied),
        "expired_token" => Poll::Failed(OAuthProblem::Expired),
        "invalid_client" | "unauthorized_client" | "invalid_grant" => {
            Poll::Failed(OAuthProblem::InvalidClient)
        }
        other => Poll::Failed(OAuthProblem::Other(other.to_string())),
    }
}

fn classify_error(error: ytmapi_rs::Error) -> Poll {
    match error.into_kind() {
        ErrorKind::InvalidResponse { response }
        | ErrorKind::UnableToSerializeGoogleOAuthToken { response, .. } => {
            match google_error(&response) {
                Some(code) => classify(&code),
                None => Poll::Failed(OAuthProblem::Other("an unreadable answer".into())),
            }
        }
        ErrorKind::Web { message } => Poll::Failed(OAuthProblem::Network(message)),
        ErrorKind::Io(error) => Poll::Failed(OAuthProblem::Network(error.to_string())),
        other => Poll::Failed(OAuthProblem::Other(other.to_string())),
    }
}

/// The code the listener confirms on Google's page.
pub struct DeviceCode {
    generator: OAuthTokenGenerator,
    /// The page with the code already filled in.
    pub url: String,
    pub user_code: String,
    started: Instant,
}

impl DeviceCode {
    fn interval(&self) -> Duration {
        Duration::from_secs(self.generator.interval.max(1) as u64)
    }

    fn deadline(&self) -> Instant {
        self.started + Duration::from_secs(self.generator.expires_in.max(1) as u64)
    }
}

/// Asks Google for a code to confirm.
pub async fn start_google_sign_in(
    client: &ytmapi_rs::Client,
    oauth: &OAuthClient,
) -> Result<DeviceCode, OAuthProblem> {
    let generator = OAuthTokenGenerator::new(client, &oauth.id)
        .await
        .map_err(|error| match classify_error(error) {
            Poll::Failed(problem) => problem,
            Poll::Pending | Poll::SlowDown => OAuthProblem::Other("an early answer".into()),
        })?;
    let url = format!(
        "{}?user_code={}",
        generator.verification_url, generator.user_code
    );
    Ok(DeviceCode {
        url,
        user_code: generator.user_code.clone(),
        generator,
        started: Instant::now(),
    })
}

/// Asks Google every `interval` seconds whether the code was confirmed,
/// until it was, was declined, ran out, or `cancel` turns true or is
/// dropped. A network failure in between is retried at the next ask.
pub async fn wait_for_google_sign_in(
    client: &ytmapi_rs::Client,
    code: &DeviceCode,
    oauth: &OAuthClient,
    mut cancel: tokio::sync::watch::Receiver<bool>,
) -> Result<OAuthToken, OAuthProblem> {
    let mut interval = code.interval();
    loop {
        if *cancel.borrow() {
            return Err(OAuthProblem::Cancelled);
        }
        if Instant::now() >= code.deadline() {
            return Err(OAuthProblem::Expired);
        }
        tokio::select! {
            () = tokio::time::sleep(interval) => {}
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
                    return Err(OAuthProblem::Cancelled);
                }
                continue;
            }
        }
        let asked = OAuthToken::from_code(
            client,
            code.generator.device_code.clone(),
            &oauth.id,
            &oauth.secret,
        )
        .await;
        match asked.map_err(classify_error) {
            Ok(token) => return Ok(token),
            Err(Poll::Pending) => {}
            Err(Poll::SlowDown) => interval += Duration::from_secs(5),
            Err(Poll::Failed(OAuthProblem::Network(message))) => {
                log::warn!("Google sign-in will ask again: {message}");
            }
            Err(Poll::Failed(problem)) => return Err(problem),
        }
    }
}

/// Where the cookie comes from.
pub const MUSIC_URL: &str = "https://music.youtube.com";

/// How long Sign in with Google waits for a browser session before giving up.
pub const BROWSER_SIGN_IN_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// How often Sign in with Google re-reads installed browsers for a session.
pub const BROWSER_SIGN_IN_POLL: Duration = Duration::from_secs(2);

/// Why browser-based Google sign-in did not finish.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrowserSignInProblem {
    /// The sign-in was cancelled here.
    Cancelled,
    /// No YouTube Music session appeared in any browser in time.
    TimedOut,
}

impl std::fmt::Display for BrowserSignInProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => write!(f, "Google sign-in was cancelled"),
            Self::TimedOut => write!(
                f,
                "no YouTube Music session found in your browsers; sign in at music.youtube.com and try again"
            ),
        }
    }
}

/// Domains whose cookies belong on a music.youtube.com request.
fn youtube_cookie_domain(domain: &str) -> bool {
    let domain = domain.trim_start_matches('.').to_ascii_lowercase();
    domain == "youtube.com"
        || domain.ends_with(".youtube.com")
        || domain == "google.com"
        || domain.ends_with(".google.com")
}

/// Cookie names that sign YouTube Music requests. Zen/Chrome often return
/// hundreds of unrelated google.com cookies; shipping all of them exceeds
/// [`MAX_COOKIE_BYTES`] and the browser sign-in path fails with TooLong.
fn music_session_cookie_name(name: &str) -> bool {
    matches!(
        name,
        "SAPISID"
            | "__Secure-1PAPISID"
            | "__Secure-3PAPISID"
            | "APISID"
            | "HSID"
            | "SSID"
            | "SID"
            | "SIDCC"
            | "__Secure-1PSID"
            | "__Secure-3PSID"
            | "__Secure-1PSIDTS"
            | "__Secure-3PSIDTS"
            | "__Secure-1PSIDCC"
            | "__Secure-3PSIDCC"
            | "LOGIN_INFO"
            | "PREF"
            | "VISITOR_INFO1_LIVE"
            | "YSC"
            | "CONSENT"
            | "SOCS"
    )
}

/// Build a Cookie header from browser cookies, or `None` if SAPISID is missing.
fn cookie_header_from_browser_cookies(cookies: &[rookie::enums::Cookie]) -> Option<String> {
    use std::collections::BTreeMap;

    // Prefer youtube.com over google.com when the same name appears in both.
    let mut pairs: BTreeMap<String, (u8, String)> = BTreeMap::new();
    for cookie in cookies {
        if cookie.value.is_empty()
            || !music_session_cookie_name(&cookie.name)
            || !youtube_cookie_domain(&cookie.domain)
        {
            continue;
        }
        let rank = if cookie
            .domain
            .trim_start_matches('.')
            .to_ascii_lowercase()
            .contains("youtube.com")
        {
            0u8
        } else {
            1u8
        };
        match pairs.get(&cookie.name) {
            Some((held, _)) if *held <= rank => {}
            _ => {
                pairs.insert(cookie.name.clone(), (rank, cookie.value.clone()));
            }
        }
    }
    if !pairs.contains_key(REQUIRED_COOKIE) {
        return None;
    }
    let header = pairs
        .into_iter()
        .map(|(name, (_, value))| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ");
    normalize_cookie(&header).ok()
}

type BrowserLoader = fn(Option<Vec<String>>) -> rookie::Result<Vec<rookie::enums::Cookie>>;

fn browser_loaders() -> Vec<BrowserLoader> {
    // Chrome / Safari first: most listeners sign in there. Each browser is
    // tried alone so one locked DB cannot block another.
    let mut loaders: Vec<BrowserLoader> = Vec::new();
    #[cfg(target_os = "macos")]
    {
        loaders.push(rookie::safari);
    }
    loaders.push(rookie::chrome);
    loaders.push(rookie::chromium);
    loaders.push(rookie::edge);
    loaders.push(rookie::brave);
    loaders.push(rookie::firefox);
    loaders.push(rookie::vivaldi);
    loaders.push(rookie::opera);
    loaders.push(rookie::arc);
    loaders.push(rookie::librewolf);
    loaders.push(rookie::zen);
    #[cfg(target_os = "macos")]
    {
        loaders.push(rookie::opera_gx);
    }
    #[cfg(target_os = "linux")]
    {
        loaders.push(rookie::cachy);
    }
    loaders
}

/// A YouTube Music cookie from an installed browser, if one is signed in.
///
/// Reads local browser cookie stores only; never logs cookie values.
pub fn youtube_music_cookie_from_browsers() -> Option<String> {
    let domains = Some(vec!["youtube.com".into(), "google.com".into()]);
    for loader in browser_loaders() {
        let Ok(cookies) = loader(domains.clone()) else {
            continue;
        };
        if let Some(header) = cookie_header_from_browser_cookies(&cookies) {
            return Some(header);
        }
    }
    None
}

/// Waits until a browser has a YouTube Music session, `cancel` turns true, or
/// [`BROWSER_SIGN_IN_TIMEOUT`] elapses.
pub async fn wait_for_browser_music_cookie(
    mut cancel: tokio::sync::watch::Receiver<bool>,
) -> Result<String, BrowserSignInProblem> {
    let deadline = Instant::now() + BROWSER_SIGN_IN_TIMEOUT;
    loop {
        if *cancel.borrow() {
            return Err(BrowserSignInProblem::Cancelled);
        }
        let found = tokio::task::spawn_blocking(youtube_music_cookie_from_browsers)
            .await
            .unwrap_or(None);
        if let Some(cookie) = found {
            return Ok(cookie);
        }
        if Instant::now() >= deadline {
            return Err(BrowserSignInProblem::TimedOut);
        }
        tokio::select! {
            () = tokio::time::sleep(BROWSER_SIGN_IN_POLL) => {}
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
                    return Err(BrowserSignInProblem::Cancelled);
                }
            }
        }
    }
}

/// The cookie `ytmapi-rs` signs requests with. Without it a pasted cookie
/// cannot sign in, however long it is.
pub const REQUIRED_COOKIE: &str = "SAPISID";

/// The longest cookie header accepted. Browsers cap a whole request header
/// block well below it; anything longer is not a cookie.
const MAX_COOKIE_BYTES: usize = 32 * 1024;

/// Why pasted text is not a YouTube Music cookie.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CookieProblem {
    /// Nothing, or only spaces, was pasted.
    Empty,
    /// The text is not `name=value; name=value` cookie pairs.
    NotACookie,
    /// A cookie, but not one from a signed-in music.youtube.com tab.
    MissingSapisid,
    /// Longer than any browser sends.
    TooLong,
}

/// The cookie header value inside what the browser's tools put on the
/// clipboard: the bare value, a `cookie: ...` line, a whole block of request
/// headers, or any of those in quotes.
fn extract(text: &str) -> &str {
    let text = text.trim();
    let from_header = text.lines().find_map(|line| {
        let line = line.trim().trim_start_matches(['\'', '"']);
        let (name, value) = line.split_once(':')?;
        name.trim().eq_ignore_ascii_case("cookie").then_some(value)
    });
    from_header
        .unwrap_or(text)
        .trim()
        .trim_matches(['\'', '"', '\\'])
        .trim()
}

/// A cookie in the form `ytmapi-rs` expects, or why it is not one. Whitespace
/// between pairs is made one space, and the last pair gets its `;`, which the
/// request signing looks for.
pub fn normalize_cookie(text: &str) -> Result<String, CookieProblem> {
    let value = extract(text);
    if value.is_empty() {
        return Err(CookieProblem::Empty);
    }
    if value.len() > MAX_COOKIE_BYTES {
        return Err(CookieProblem::TooLong);
    }
    let mut pairs = Vec::new();
    for pair in value.split(';').map(str::trim).filter(|p| !p.is_empty()) {
        let Some((name, data)) = pair.split_once('=') else {
            return Err(CookieProblem::NotACookie);
        };
        let name = name.trim();
        let valid = !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_graphic() && !b"()<>@,;:\\\"/[]?={}".contains(&byte))
            && data
                .bytes()
                .all(|byte| byte.is_ascii_graphic() || byte == b' ');
        if !valid {
            return Err(CookieProblem::NotACookie);
        }
        pairs.push(format!("{name}={}", data.trim()));
    }
    if pairs.is_empty() {
        return Err(CookieProblem::NotACookie);
    }
    if !pairs
        .iter()
        .any(|pair| pair.starts_with(&format!("{REQUIRED_COOKIE}=")))
    {
        return Err(CookieProblem::MissingSapisid);
    }
    Ok(format!("{};", pairs.join("; ")))
}

/// Whether `text` could be pasted as the cookie, for the sign-in button.
pub fn looks_like_cookie(text: &str) -> bool {
    normalize_cookie(text).is_ok()
}

/// A grant a version before YouTube Music signed in with Spotify, kept only
/// so the credential store can read the files such a version left and the
/// backend can delete them. Deliberately has no Debug implementation.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StoredToken {
    pub client_id: String,
    pub access_token: String,
    pub refresh_token: String,
    /// Unix seconds.
    pub expires_at: u64,
    #[serde(default)]
    pub scope: String,
}

impl StoredToken {
    pub fn load(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }

    // Legacy file fixtures only. Production persistence lives in credentials.rs.
    #[cfg(test)]
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self)?;
        let temporary = path.with_extension("json.tmp");
        write_private(&temporary, text.as_bytes())?;
        std::fs::rename(&temporary, path)?;
        Ok(())
    }

    // Legacy file fixtures only. Production persistence lives in credentials.rs.
    #[cfg(test)]
    pub fn remove(path: &Path) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(contents)?;
    file.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAIRS: &str =
        "VISITOR_INFO1_LIVE=abc; SAPISID=dummy-sapisid/xyz; __Secure-3PAPISID=dummy";

    #[test]
    fn a_bare_cookie_gets_the_trailing_semicolon_request_signing_needs() {
        let cookie = normalize_cookie(PAIRS).unwrap();
        assert_eq!(cookie, format!("{PAIRS};"));
        assert_eq!(normalize_cookie(&cookie).unwrap(), cookie);
    }

    #[test]
    fn the_cookie_is_found_in_a_pasted_header_line_or_block() {
        let wanted = format!("{PAIRS};");
        assert_eq!(
            normalize_cookie(&format!("Cookie: {PAIRS}")).unwrap(),
            wanted
        );
        assert_eq!(
            normalize_cookie(&format!("'cookie: {PAIRS}'")).unwrap(),
            wanted
        );
        let block = format!(
            "accept: */*\r\nauthorization: SAPISIDHASH dummy\r\ncookie: {PAIRS}\r\norigin: https://music.youtube.com\r\n"
        );
        assert_eq!(normalize_cookie(&block).unwrap(), wanted);
        assert_eq!(
            normalize_cookie(&format!("  \"{PAIRS}\"\n")).unwrap(),
            wanted
        );
    }

    #[test]
    fn text_that_cannot_sign_in_says_why() {
        assert_eq!(normalize_cookie("  \n"), Err(CookieProblem::Empty));
        assert_eq!(normalize_cookie("hello"), Err(CookieProblem::NotACookie));
        assert_eq!(normalize_cookie("a=b; c d"), Err(CookieProblem::NotACookie));
        assert_eq!(
            normalize_cookie("a=b; c=d"),
            Err(CookieProblem::MissingSapisid)
        );
        assert_eq!(
            normalize_cookie("a=b; XSAPISID=c"),
            Err(CookieProblem::MissingSapisid),
            "only the SAPISID cookie itself counts"
        );
        assert_eq!(
            normalize_cookie(&format!("SAPISID={}", "x".repeat(MAX_COOKIE_BYTES))),
            Err(CookieProblem::TooLong)
        );
    }

    #[test]
    fn a_cookie_value_cannot_smuggle_a_second_header() {
        assert!(normalize_cookie("SAPISID=a\r\nx-evil: 1").is_err());
        assert!(!looks_like_cookie("SAPISID=a\u{7f}b"));
    }

    const TOKEN: &str = r#"{"token_type":"Bearer","access_token":"dummy-access","refresh_token":"dummy-refresh","expires_in":3599,"request_time":{"secs_since_epoch":1700000000,"nanos_since_epoch":0},"client_id":"dummy-id","client_secret":"dummy-secret"}"#;

    #[test]
    fn an_oauth_token_round_trips_and_is_never_taken_for_a_cookie() {
        let token = decode_oauth_token(TOKEN).expect("the stored token reads back");
        let encoded = encode_oauth_token(&token);
        assert!(is_oauth_blob(&encoded));
        assert_eq!(decode_oauth_token(&encoded), Some(token.clone()));
        assert_eq!(oauth_expires_at(&token), Some(1_700_003_599));
        assert!(!looks_like_cookie(&encoded));
        assert!(!is_oauth_blob(PAIRS));
        assert_eq!(decode_oauth_token(PAIRS), None);
        assert_eq!(decode_oauth_token("{not json"), None);
    }

    #[test]
    fn a_stored_credential_signs_in_as_a_cookie_or_a_token() {
        assert_eq!(stored_sign_in(PAIRS), Some(format!("{PAIRS};")));
        assert_eq!(stored_sign_in(&format!(" {TOKEN}\n")), Some(TOKEN.into()));
        assert_eq!(stored_sign_in("{\"spotify\":true}"), None);
        assert_eq!(stored_sign_in("dummy-streaming-token"), None);
    }

    #[test]
    fn settings_name_the_oauth_client_before_the_environment() {
        let env = |name: &str| match name {
            "YTMFAST_OAUTH_CLIENT_ID" => Some("env-id".to_string()),
            "YOUTUI_OAUTH_CLIENT_SECRET" => Some("youtui-secret".to_string()),
            _ => None,
        };
        let client = oauth_client_from(Some(" set-id "), None, env).unwrap();
        assert_eq!(client.id, "set-id");
        assert_eq!(client.secret, "youtui-secret");
        let client = oauth_client_from(Some(""), Some("set-secret"), env).unwrap();
        assert_eq!(client.id, "env-id");
        assert_eq!(client.secret, "set-secret");
        assert!(oauth_client_from(Some("id"), None, |_| None).is_none());
    }

    #[test]
    fn google_waiting_answers_say_whether_to_keep_asking() {
        let answer =
            |code: &str| classify(&google_error(&format!(r#"{{"error":"{code}"}}"#)).unwrap());
        assert_eq!(answer("authorization_pending"), Poll::Pending);
        assert_eq!(answer("slow_down"), Poll::SlowDown);
        assert_eq!(answer("access_denied"), Poll::Failed(OAuthProblem::Denied));
        assert_eq!(answer("expired_token"), Poll::Failed(OAuthProblem::Expired));
        assert_eq!(
            answer("invalid_client"),
            Poll::Failed(OAuthProblem::InvalidClient)
        );
        assert_eq!(google_error("not json"), None);
    }

    fn sample_cookie(domain: &str, name: &str, value: &str) -> rookie::enums::Cookie {
        rookie::enums::Cookie {
            domain: domain.into(),
            path: "/".into(),
            secure: true,
            expires: None,
            name: name.into(),
            value: value.into(),
            http_only: false,
            same_site: 0,
        }
    }

    #[test]
    fn a_browser_cookie_jar_needs_sapisid_on_youtube_or_google() {
        assert!(cookie_header_from_browser_cookies(&[sample_cookie(
            ".youtube.com",
            "SID",
            "x"
        )])
        .is_none());
        let header = cookie_header_from_browser_cookies(&[
            sample_cookie(".google.com", "SAPISID", "from-google"),
            sample_cookie(".youtube.com", "SAPISID", "from-youtube"),
            sample_cookie(".youtube.com", "HSID", "h"),
        ])
        .expect("SAPISID is enough");
        assert!(header.contains("SAPISID=from-youtube"));
        assert!(header.contains("HSID=h"));
        assert!(!header.contains("from-google"));
    }

    #[test]
    fn a_crowded_browser_jar_keeps_only_music_session_cookies() {
        let mut cookies = vec![
            sample_cookie(".youtube.com", "SAPISID", "sap"),
            sample_cookie(".youtube.com", "HSID", "h"),
        ];
        for i in 0..200 {
            cookies.push(sample_cookie(
                ".google.com",
                &format!("JUNK{i}"),
                &"x".repeat(200),
            ));
        }
        let header =
            cookie_header_from_browser_cookies(&cookies).expect("session cookies stay short");
        assert!(header.contains("SAPISID=sap"));
        assert!(!header.contains("JUNK"));
        assert!(header.len() < MAX_COOKIE_BYTES);
    }
}

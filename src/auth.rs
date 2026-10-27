//! YouTube Music sign-in with the browser cookie.
//!
//! Google offers no sign-in this app can drive, so the listener signs in the
//! way `ytmapi-rs` and youtui document: copy the `cookie` request header of
//! any request to music.youtube.com from the browser's developer tools and
//! paste it into the sign-in screen. The cookie is a bearer secret. It is
//! sent only to YouTube, kept only in the protected credential store as
//! [`crate::player::Credentials::with_cookie`], and never logged.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Where the cookie comes from.
pub const MUSIC_URL: &str = "https://music.youtube.com";

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
}

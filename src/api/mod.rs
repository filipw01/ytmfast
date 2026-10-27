//! The catalogue and account client: YouTube Music through `ytmapi-rs`.

use std::fmt;

pub mod client;
pub mod gateway;
pub mod models;
pub mod ytm;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ApiSource {
    Shared,
    Personal,
}

impl fmt::Display for ApiSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Shared => formatter.write_str("shared"),
            Self::Personal => formatter.write_str("personal"),
        }
    }
}

pub use client::{ApiClient, ApiError, CookieSession, NetActivity, PlayRequest, TokenProvider};
pub use gateway::{AccountId, ApiGateway, Operation, PlaylistAccess, PlaylistId, SessionState};

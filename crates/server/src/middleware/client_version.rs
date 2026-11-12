//! `X-Client-Version` 檢查（`docs/architecture.md` §10）。
//!
//! - 版本低於 [`MIN_CLIENT_VERSION`] → `426 CLIENT_OUTDATED`
//! - 版本格式錯誤 → `400 BAD_REQUEST`
//! - 沒帶標頭 → 放行（方便用 curl 除錯）

use std::sync::LazyLock;

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use hammer_shared::ErrorCode;
use hammer_shared::constants::{CLIENT_VERSION_HEADER, MIN_CLIENT_VERSION};
use semver::Version;
use serde_json::json;

use crate::AppError;

static MIN_VERSION: LazyLock<Version> = LazyLock::new(|| {
    Version::parse(MIN_CLIENT_VERSION).expect("MIN_CLIENT_VERSION 必須是合法的 semver")
});

/// 檢查結果。
#[derive(Debug, PartialEq, Eq)]
pub enum Check {
    /// 放行。
    Allowed,
    /// 版本過舊。
    Outdated(Version),
    /// 標頭格式錯誤。
    Invalid,
}

/// 檢查標頭值（`None` 代表沒帶標頭）。
pub fn check(header: Option<&[u8]>) -> Check {
    let Some(raw) = header else {
        return Check::Allowed;
    };
    let parsed = std::str::from_utf8(raw)
        .ok()
        .and_then(|s| Version::parse(s.trim()).ok());
    match parsed {
        None => Check::Invalid,
        Some(v) if v < *MIN_VERSION => Check::Outdated(v),
        Some(_) => Check::Allowed,
    }
}

/// axum 中介層。
pub async fn layer(req: Request, next: Next) -> Response {
    let header = req
        .headers()
        .get(CLIENT_VERSION_HEADER)
        .map(|v| v.as_bytes());
    match check(header) {
        Check::Allowed => next.run(req).await,
        Check::Outdated(version) => AppError::new(
            ErrorCode::ClientOutdated,
            crate::error::default_message(ErrorCode::ClientOutdated),
        )
        .with_details(json!({
            "client_version": version.to_string(),
            "min_version": MIN_CLIENT_VERSION,
        }))
        .into_response(),
        Check::Invalid => AppError::new(
            ErrorCode::BadRequest,
            format!("{CLIENT_VERSION_HEADER} 標頭格式錯誤，應為 semver（例如 1.2.3）"),
        )
        .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_version_is_valid_semver() {
        LazyLock::force(&MIN_VERSION);
    }

    #[test]
    fn missing_header_is_allowed() {
        assert_eq!(check(None), Check::Allowed);
    }

    #[test]
    fn equal_or_newer_is_allowed() {
        assert_eq!(check(Some(MIN_CLIENT_VERSION.as_bytes())), Check::Allowed);
        assert_eq!(check(Some(b"99.0.0")), Check::Allowed);
    }

    #[test]
    fn older_is_outdated() {
        // 預發行版本小於正式版，因此 0.1.0-alpha 低於 0.1.0
        let older = format!("{}-alpha", *MIN_VERSION);
        assert!(matches!(check(Some(older.as_bytes())), Check::Outdated(_)));
        assert!(matches!(check(Some(b"0.0.1")), Check::Outdated(_)));
    }

    #[test]
    fn invalid_is_rejected() {
        assert_eq!(check(Some(b"abc")), Check::Invalid);
        assert_eq!(check(Some(b"1.2")), Check::Invalid);
        assert_eq!(check(Some(&[0xff, 0xfe])), Check::Invalid);
    }
}

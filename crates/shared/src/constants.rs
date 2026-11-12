//! 版本相容資訊與 HTTP 標頭名稱（`docs/architecture.md` §10）。

/// 客戶端在每個請求帶上自己的版本號所用的標頭。
pub const CLIENT_VERSION_HEADER: &str = "X-Client-Version";

/// 伺服器接受的最低客戶端版本（semver）。
///
/// 客戶端版本低於此值時，伺服器回傳 `426`（[`crate::ErrorCode::ClientOutdated`]）。
/// 有不相容的 API 變更時調高。
pub const MIN_CLIENT_VERSION: &str = "0.1.0";

/// API 路徑前綴。
pub const API_PREFIX: &str = "/api/v1";

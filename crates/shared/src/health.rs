//! `GET /api/v1/health` 的回應格式。
//!
//! API Server 回傳、桌面程式解析都使用這裡的型別，確保兩邊一致。

use serde::{Deserialize, Serialize};
use specta::Type;

/// 健康狀態。JSON 中為小寫字串（`"ok"`、`"error"`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Ok,
    Error,
}

/// `GET /api/v1/health` 的回應。
///
/// 資料庫正常時 HTTP `200`；資料庫異常時 HTTP `503`，內容仍是本型別。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct HealthResponse {
    /// 整體狀態。
    pub status: HealthStatus,
    /// 伺服器版本。
    pub version: String,
    /// 資料庫連線狀態。
    pub database: HealthStatus,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn json_shape() {
        let body = HealthResponse {
            status: HealthStatus::Error,
            version: "0.1.0".to_owned(),
            database: HealthStatus::Error,
        };
        assert_eq!(
            serde_json::to_value(&body).unwrap(),
            json!({ "status": "error", "version": "0.1.0", "database": "error" })
        );
    }
}

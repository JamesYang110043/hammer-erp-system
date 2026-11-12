//! API Server 的 HTTP client（`docs/architecture.md` §3.3）。
//!
//! - 每個請求自動帶 `X-Client-Version` 標頭（§10）
//! - 伺服器回傳的錯誤轉成 shared 的 [`ApiError`]

use std::time::Duration;

use hammer_shared::ApiError;
use hammer_shared::constants::{API_PREFIX, CLIENT_VERSION_HEADER};
use hammer_shared::health::HealthResponse;
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::{Response, StatusCode};
use serde::de::DeserializeOwned;

use crate::error::ClientError;

/// 桌面程式版本（`Cargo.toml` 的 `version`），送給伺服器做版本檢查。
pub const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
pub struct ApiClient {
    http: reqwest::Client,
}

impl ApiClient {
    pub fn new() -> reqwest::Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(
            CLIENT_VERSION_HEADER,
            HeaderValue::from_static(CLIENT_VERSION),
        );
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .user_agent(concat!("hammer-desktop/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()?;
        Ok(Self { http })
    }

    /// `GET /api/v1/health`。
    ///
    /// 資料庫異常時伺服器回 `503`，內容仍是 [`HealthResponse`]，這裡當作成功回傳，
    /// 由呼叫端看 `database` 欄位判斷。
    pub async fn health(&self, server_url: &str) -> Result<HealthResponse, ClientError> {
        let resp = self.get(server_url, "/health").await?;
        if resp.status() == StatusCode::SERVICE_UNAVAILABLE {
            let status = resp.status();
            let body = read_body(resp).await?;
            return serde_json::from_slice(&body).map_err(|_| error_from_body(status, &body));
        }
        read_json(resp).await
    }

    async fn get(&self, server_url: &str, path: &str) -> Result<Response, ClientError> {
        self.http
            .get(endpoint(server_url, path))
            .send()
            .await
            .map_err(network_error)
    }
}

/// `http://host:8080` + `/health` → `http://host:8080/api/v1/health`。
fn endpoint(server_url: &str, path: &str) -> String {
    format!("{}{API_PREFIX}{path}", server_url.trim_end_matches('/'))
}

/// 成功時解析成 `T`；失敗時轉成 [`ClientError`]。
async fn read_json<T: DeserializeOwned>(resp: Response) -> Result<T, ClientError> {
    let status = resp.status();
    let body = read_body(resp).await?;
    if !status.is_success() {
        return Err(error_from_body(status, &body));
    }
    serde_json::from_slice(&body).map_err(|_| ClientError::InvalidResponse {
        message: format!(
            "伺服器回應格式不正確（HTTP {}），請確認伺服器網址",
            status.as_u16()
        ),
    })
}

async fn read_body(resp: Response) -> Result<Vec<u8>, ClientError> {
    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(network_error)
}

/// 錯誤回應：統一錯誤格式 → [`ClientError::Api`]；其他 → [`ClientError::InvalidResponse`]。
fn error_from_body(status: StatusCode, body: &[u8]) -> ClientError {
    match serde_json::from_slice::<ApiError>(body) {
        Ok(err) => ClientError::Api(err),
        Err(_) => ClientError::InvalidResponse {
            message: format!(
                "伺服器回應錯誤（HTTP {}），請確認伺服器網址",
                status.as_u16()
            ),
        },
    }
}

fn network_error(err: reqwest::Error) -> ClientError {
    let message = if err.is_timeout() {
        "連線逾時，請確認伺服器是否正常運作"
    } else if err.is_connect() {
        "無法連線到伺服器，請確認伺服器已啟動、網址正確"
    } else {
        "與伺服器通訊失敗"
    };
    ClientError::Network {
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::http::{HeaderMap as AxumHeaders, StatusCode as AxumStatus};
    use axum::routing::get;
    use hammer_shared::ErrorCode;
    use hammer_shared::health::HealthStatus;
    use serde_json::json;

    /// 在隨機埠啟動測試用伺服器，回傳網址。
    async fn serve(api: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().nest(API_PREFIX, api);
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    fn health_body(database: &str) -> serde_json::Value {
        json!({ "status": database, "version": "9.9.9", "database": database })
    }

    #[test]
    fn endpoint_joins_prefix() {
        assert_eq!(
            endpoint("http://localhost:8080", "/health"),
            "http://localhost:8080/api/v1/health"
        );
        assert_eq!(
            endpoint("http://h/erp/", "/health"),
            "http://h/erp/api/v1/health"
        );
    }

    #[tokio::test]
    async fn health_ok_and_sends_client_version() {
        let url = serve(Router::new().route(
            "/health",
            get(|headers: AxumHeaders| async move {
                let version = headers
                    .get(CLIENT_VERSION_HEADER)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_owned();
                // 把收到的版本放進 version 欄位，方便驗證
                axum::Json(json!({ "status": "ok", "version": version, "database": "ok" }))
            }),
        ))
        .await;

        let health = ApiClient::new().unwrap().health(&url).await.unwrap();
        assert_eq!(health.status, HealthStatus::Ok);
        assert_eq!(health.version, CLIENT_VERSION);
    }

    #[tokio::test]
    async fn health_503_is_returned_as_database_error() {
        let url = serve(Router::new().route(
            "/health",
            get(|| async {
                (
                    AxumStatus::SERVICE_UNAVAILABLE,
                    axum::Json(health_body("error")),
                )
            }),
        ))
        .await;

        let health = ApiClient::new().unwrap().health(&url).await.unwrap();
        assert_eq!(health.database, HealthStatus::Error);
        assert_eq!(health.version, "9.9.9");
    }

    #[tokio::test]
    async fn server_error_becomes_api_error() {
        let url = serve(Router::new().route(
            "/health",
            get(|| async {
                (
                    AxumStatus::UPGRADE_REQUIRED,
                    axum::Json(json!({
                        "code": "CLIENT_OUTDATED",
                        "message": "請更新程式",
                        "details": { "min_version": "1.0.0" }
                    })),
                )
            }),
        ))
        .await;

        let err = ApiClient::new().unwrap().health(&url).await.unwrap_err();
        let ClientError::Api(api) = err else {
            panic!("應該是 ApiError：{err:?}");
        };
        assert_eq!(api.code, ErrorCode::ClientOutdated);
        assert_eq!(api.details, Some(json!({ "min_version": "1.0.0" })));
    }

    #[tokio::test]
    async fn non_json_response_is_invalid_response() {
        let url = serve(Router::new().route("/health", get(|| async { "hello" }))).await;
        let err = ApiClient::new().unwrap().health(&url).await.unwrap_err();
        assert!(
            matches!(err, ClientError::InvalidResponse { .. }),
            "{err:?}"
        );

        // 404 且不是統一錯誤格式（例如網址指到別的服務）
        let url = serve(Router::new()).await;
        let err = ApiClient::new().unwrap().health(&url).await.unwrap_err();
        assert!(
            matches!(err, ClientError::InvalidResponse { .. }),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn unreachable_server_is_network_error() {
        // 綁定後立刻關閉，取得一個沒有人在聽的埠
        let addr = {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            listener.local_addr().unwrap()
        };
        let err = ApiClient::new()
            .unwrap()
            .health(&format!("http://{addr}"))
            .await
            .unwrap_err();
        assert!(matches!(err, ClientError::Network { .. }), "{err:?}");
    }
}

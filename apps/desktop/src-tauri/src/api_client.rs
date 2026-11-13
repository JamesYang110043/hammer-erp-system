//! API Server 的 HTTP client（`docs/architecture.md` §3.3）。
//!
//! - 每個請求自動帶 `X-Client-Version` 標頭（§10）
//! - 需要登入的請求帶 `Authorization: Bearer <token>`；伺服器回 `401` 時轉成
//!   [`ClientError::SessionExpired`]
//! - 伺服器回傳的錯誤轉成 shared 的 [`ApiError`]

use std::time::Duration;

use hammer_shared::auth::{ChangePasswordRequest, CurrentUser, LoginRequest, LoginResponse};
use hammer_shared::constants::{API_PREFIX, CLIENT_VERSION_HEADER};
use hammer_shared::health::HealthResponse;
use hammer_shared::setup::{CreateAdminRequest, SetupStatus};
use hammer_shared::{ApiError, ErrorCode};
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::{RequestBuilder, Response, StatusCode};
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
        let resp = send(self.http.get(endpoint(server_url, "/health"))).await?;
        if resp.status() == StatusCode::SERVICE_UNAVAILABLE {
            let status = resp.status();
            let body = read_body(resp).await?;
            return serde_json::from_slice(&body).map_err(|_| error_from_body(status, &body));
        }
        read_json(resp).await
    }

    /// `GET /api/v1/setup/status`（不需要登入）。
    pub async fn setup_status(&self, server_url: &str) -> Result<SetupStatus, ClientError> {
        read_json(send(self.http.get(endpoint(server_url, "/setup/status"))).await?).await
    }

    /// `POST /api/v1/setup/admin`（不需要登入）。
    pub async fn setup_admin(
        &self,
        server_url: &str,
        req: &CreateAdminRequest,
    ) -> Result<LoginResponse, ClientError> {
        let builder = self
            .http
            .post(endpoint(server_url, "/setup/admin"))
            .json(req);
        read_json(send(builder).await?).await
    }

    /// `POST /api/v1/auth/login`（不需要登入）。
    ///
    /// 帳號或密碼錯誤時伺服器回 `401`，這裡是 [`ClientError::Api`]，不是
    /// [`ClientError::SessionExpired`]。
    pub async fn login(
        &self,
        server_url: &str,
        req: &LoginRequest,
    ) -> Result<LoginResponse, ClientError> {
        let builder = self
            .http
            .post(endpoint(server_url, "/auth/login"))
            .json(req);
        read_json(send(builder).await?).await
    }

    /// `POST /api/v1/auth/logout`。
    pub async fn logout(&self, server_url: &str, token: &str) -> Result<(), ClientError> {
        let builder = self
            .http
            .post(endpoint(server_url, "/auth/logout"))
            .bearer_auth(token);
        authed(read_empty(send(builder).await?).await)
    }

    /// `GET /api/v1/auth/me`。
    pub async fn me(&self, server_url: &str, token: &str) -> Result<CurrentUser, ClientError> {
        let builder = self
            .http
            .get(endpoint(server_url, "/auth/me"))
            .bearer_auth(token);
        authed(read_json(send(builder).await?).await)
    }

    /// `POST /api/v1/auth/change-password`。
    pub async fn change_password(
        &self,
        server_url: &str,
        token: &str,
        req: &ChangePasswordRequest,
    ) -> Result<(), ClientError> {
        let builder = self
            .http
            .post(endpoint(server_url, "/auth/change-password"))
            .bearer_auth(token)
            .json(req);
        authed(read_empty(send(builder).await?).await)
    }
}

async fn send(builder: RequestBuilder) -> Result<Response, ClientError> {
    builder.send().await.map_err(network_error)
}

/// 需要登入的請求：伺服器回 `401 UNAUTHORIZED` 表示 session 已失效。
fn authed<T>(result: Result<T, ClientError>) -> Result<T, ClientError> {
    match result {
        Err(ClientError::Api(ApiError {
            code: ErrorCode::Unauthorized,
            ..
        })) => Err(ClientError::session_expired()),
        other => other,
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

/// 沒有回應內容的端點（`204 No Content`）。
async fn read_empty(resp: Response) -> Result<(), ClientError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let body = read_body(resp).await?;
    Err(error_from_body(status, &body))
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
    use axum::routing::{get, post};
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

    fn user_json() -> serde_json::Value {
        json!({
            "id": "0196f5a0-0000-7000-8000-000000000001",
            "username": "admin",
            "display_name": "管理者",
            "role": "admin",
            "permissions": ["user.manage"],
            "must_change_password": false,
        })
    }

    fn bearer(headers: &AxumHeaders) -> String {
        headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    }

    fn unauthorized(message: &str) -> (AxumStatus, axum::Json<serde_json::Value>) {
        (
            AxumStatus::UNAUTHORIZED,
            axum::Json(json!({ "code": "UNAUTHORIZED", "message": message })),
        )
    }

    #[tokio::test]
    async fn login_posts_credentials_and_returns_token() {
        let url = serve(Router::new().route(
            "/auth/login",
            post(
                |axum::Json(body): axum::Json<serde_json::Value>| async move {
                    assert_eq!(body, json!({ "username": "admin", "password": "pw" }));
                    axum::Json(json!({ "token": "tok", "user": user_json() }))
                },
            ),
        ))
        .await;

        let resp = ApiClient::new()
            .unwrap()
            .login(
                &url,
                &LoginRequest {
                    username: "admin".into(),
                    password: "pw".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(resp.token, "tok");
        assert_eq!(resp.user.username, "admin");
    }

    #[tokio::test]
    async fn login_401_stays_api_error() {
        // 帳號或密碼錯誤不是 session 逾時
        let url = serve(Router::new().route(
            "/auth/login",
            post(|| async { unauthorized("帳號或密碼錯誤") }),
        ))
        .await;
        let err = ApiClient::new()
            .unwrap()
            .login(
                &url,
                &LoginRequest {
                    username: "a".into(),
                    password: "b".into(),
                },
            )
            .await
            .unwrap_err();
        let ClientError::Api(api) = err else {
            panic!("應該是 ApiError：{err:?}");
        };
        assert_eq!(api.code, ErrorCode::Unauthorized);
        assert_eq!(api.message, "帳號或密碼錯誤");
    }

    #[tokio::test]
    async fn me_sends_bearer_token() {
        let url = serve(Router::new().route(
            "/auth/me",
            get(|headers: AxumHeaders| async move {
                assert_eq!(bearer(&headers), "Bearer tok");
                axum::Json(user_json())
            }),
        ))
        .await;
        let user = ApiClient::new().unwrap().me(&url, "tok").await.unwrap();
        assert_eq!(user.display_name, "管理者");
    }

    #[tokio::test]
    async fn authed_401_becomes_session_expired() {
        let url = serve(
            Router::new()
                .route("/auth/me", get(|| async { unauthorized("請先登入") }))
                .route(
                    "/auth/change-password",
                    post(|| async { unauthorized("請先登入") }),
                ),
        )
        .await;
        let client = ApiClient::new().unwrap();
        assert_eq!(
            client.me(&url, "old").await.unwrap_err(),
            ClientError::session_expired()
        );
        let req = ChangePasswordRequest {
            current_password: "a".into(),
            new_password: "b".into(),
        };
        assert_eq!(
            client.change_password(&url, "old", &req).await.unwrap_err(),
            ClientError::session_expired()
        );
    }

    #[tokio::test]
    async fn no_content_endpoints() {
        let url = serve(
            Router::new()
                .route(
                    "/auth/logout",
                    post(|headers: AxumHeaders| async move {
                        assert_eq!(bearer(&headers), "Bearer tok");
                        AxumStatus::NO_CONTENT
                    }),
                )
                .route(
                    "/auth/change-password",
                    post(|| async {
                        (
                            AxumStatus::UNPROCESSABLE_ENTITY,
                            axum::Json(
                                json!({ "code": "VALIDATION", "message": "目前的密碼不正確" }),
                            ),
                        )
                    }),
                ),
        )
        .await;
        let client = ApiClient::new().unwrap();
        client.logout(&url, "tok").await.unwrap();

        let req = ChangePasswordRequest {
            current_password: "a".into(),
            new_password: "b".into(),
        };
        let err = client.change_password(&url, "tok", &req).await.unwrap_err();
        let ClientError::Api(api) = err else {
            panic!("應該是 ApiError：{err:?}");
        };
        assert_eq!(api.code, ErrorCode::Validation);
    }

    #[tokio::test]
    async fn setup_endpoints() {
        let url = serve(
            Router::new()
                .route(
                    "/setup/status",
                    get(|| async { axum::Json(json!({ "needs_setup": true })) }),
                )
                .route(
                    "/setup/admin",
                    post(
                        |axum::Json(body): axum::Json<serde_json::Value>| async move {
                            assert_eq!(body["display_name"], "管理者");
                            axum::Json(json!({ "token": "first", "user": user_json() }))
                        },
                    ),
                ),
        )
        .await;
        let client = ApiClient::new().unwrap();
        assert!(client.setup_status(&url).await.unwrap().needs_setup);
        let resp = client
            .setup_admin(
                &url,
                &CreateAdminRequest {
                    username: "admin".into(),
                    display_name: "管理者".into(),
                    password: "0123456789".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(resp.token, "first");
    }
}

//! 初次啟動建立第一個管理者 API 的整合測試（#26）。
//!
//! 使用 `#[sqlx::test]`，需要 `DATABASE_URL`（見 README「整合測試」）。
//! 透過完整的 router 呼叫，包含所有中介層。

#![allow(clippy::unwrap_used)]

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use hammer_core::test_util::TestUser;
use hammer_server::{AppState, Config, router};
use hammer_shared::auth::{CurrentUser, LoginResponse};
use hammer_shared::setup::SetupStatus;
use hammer_shared::{ApiError, ErrorCode, Id, audit, permission, role};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

const PASSWORD: &str = "correct horse battery";

fn app(pool: &PgPool) -> Router {
    let config = Config::from_sources(None, |name| match name {
        "DATABASE_URL" => Some("postgres://unused".to_owned()),
        _ => None,
    })
    .unwrap();
    router(AppState::new(pool.clone(), config))
}

async fn call(
    pool: &PgPool,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::USER_AGENT, "HammerERP-test")
        .header("X-Client-Version", "0.1.0");
    if let Some(token) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let body = match body {
        Some(json) => {
            req = req.header(header::CONTENT_TYPE, "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };
    let res = app(pool).oneshot(req.body(body).unwrap()).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, value)
}

async fn status(pool: &PgPool) -> SetupStatus {
    let (status, body) = call(pool, Method::GET, "/api/v1/setup/status", None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn create_admin(
    pool: &PgPool,
    username: &str,
    display_name: &str,
    password: &str,
) -> (StatusCode, Value) {
    call(
        pool,
        Method::POST,
        "/api/v1/setup/admin",
        None,
        Some(json!({ "username": username, "display_name": display_name, "password": password })),
    )
    .await
}

async fn create_admin_ok(pool: &PgPool) -> LoginResponse {
    let (status, body) = create_admin(pool, "admin", "系統管理者", PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

fn assert_error(status: StatusCode, body: &Value, expected: ErrorCode) -> ApiError {
    assert_eq!(status.as_u16(), expected.http_status(), "{body}");
    let err: ApiError = serde_json::from_value(body.clone()).unwrap();
    assert_eq!(err.code, expected, "{body}");
    assert!(!err.message.is_empty());
    err
}

async fn user_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(pool)
        .await
        .unwrap()
}

// ---------------------------------------------------------------------------
// 驗收條件
// ---------------------------------------------------------------------------

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn needs_setup_until_admin_created(pool: PgPool) {
    assert_eq!(status(&pool).await, SetupStatus { needs_setup: true });
    create_admin_ok(&pool).await;
    assert_eq!(status(&pool).await, SetupStatus { needs_setup: false });
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn second_create_is_conflict(pool: PgPool) {
    create_admin_ok(&pool).await;

    let (status, body) = create_admin(&pool, "other", "Other", PASSWORD).await;
    let err = assert_error(status, &body, ErrorCode::Conflict);
    assert_eq!(err.message, "系統已完成初始設定，請直接登入");
    assert_eq!(user_count(&pool).await, 1);

    // 已設定過時，輸入不合法也回傳 409（不透露驗證規則、不做密碼雜湊）
    let (status, body) = create_admin(&pool, "", "", "short").await;
    assert_error(status, &body, ErrorCode::Conflict);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn concurrent_creates_only_one_succeeds(pool: PgPool) {
    let (a, b) = tokio::join!(
        create_admin(&pool, "admin-a", "A", PASSWORD),
        create_admin(&pool, "admin-b", "B", PASSWORD),
    );
    let mut statuses = [a.0, b.0];
    statuses.sort();
    assert_eq!(
        statuses,
        [StatusCode::OK, StatusCode::CONFLICT],
        "{a:?} {b:?}"
    );
    let failed = if a.0 == StatusCode::CONFLICT { &a } else { &b };
    assert_error(failed.0, &failed.1, ErrorCode::Conflict);
    assert_eq!(user_count(&pool).await, 1);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn returned_token_is_admin(pool: PgPool) {
    let res = create_admin_ok(&pool).await;
    assert_eq!(res.token.len(), 43);
    assert_eq!(res.user.username, "admin");
    assert_eq!(res.user.display_name, "系統管理者");
    assert_eq!(res.user.role, role::ADMIN);
    assert!(!res.user.must_change_password);
    let mut expected: Vec<&str> = permission::ALL.to_vec();
    expected.sort_unstable();
    assert_eq!(res.user.permissions, expected);

    let (status, body) = call(
        &pool,
        Method::GET,
        "/api/v1/auth/me",
        Some(&res.token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let me: CurrentUser = serde_json::from_value(body).unwrap();
    assert_eq!(me, res.user);
    assert_eq!(me.role, role::ADMIN);
}

// ---------------------------------------------------------------------------
// 其他行為
// ---------------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
struct AuditRow {
    user_id: Option<Id>,
    action: String,
    entity_key: Option<String>,
    record_id: Option<Id>,
    before_data: Option<Value>,
    after_data: Option<Value>,
    client_info: Option<String>,
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn writes_create_audit_log_without_password(pool: PgPool) {
    let res = create_admin_ok(&pool).await;

    let logs: Vec<AuditRow> = sqlx::query_as(
        "SELECT user_id, action, entity_key, record_id, before_data, after_data, client_info
         FROM audit_logs",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(logs.len(), 1, "{logs:?}");
    let log = &logs[0];
    assert_eq!(log.action, audit::CREATE);
    assert_eq!(log.user_id, Some(res.user.id));
    assert_eq!(log.entity_key.as_deref(), Some("user"));
    assert_eq!(log.record_id, Some(res.user.id));
    assert_eq!(log.before_data, None);
    assert_eq!(
        log.after_data,
        Some(json!({
            "username": "admin",
            "display_name": "系統管理者",
            "role": role::ADMIN,
            "must_change_password": false,
        }))
    );
    assert_eq!(log.client_info.as_deref(), Some("HammerERP-test / 0.1.0"));
    let text = log.after_data.as_ref().unwrap().to_string();
    assert!(!text.contains(PASSWORD) && !text.contains("argon2"));
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn created_admin_can_log_in(pool: PgPool) {
    let res = create_admin_ok(&pool).await;

    let (must_change, version): (bool, i32) =
        sqlx::query_as("SELECT must_change_password, version FROM users WHERE id = $1")
            .bind(res.user.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!must_change);
    assert_eq!(version, 1);

    let (status, body) = call(
        &pool,
        Method::POST,
        "/api/v1/auth/login",
        None,
        Some(json!({ "username": "admin", "password": PASSWORD })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn input_is_trimmed(pool: PgPool) {
    let (status, body) = create_admin(&pool, "  boss ", " 老闆 ", PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let res: LoginResponse = serde_json::from_value(body).unwrap();
    assert_eq!(res.user.username, "boss");
    assert_eq!(res.user.display_name, "老闆");
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn invalid_input_is_rejected(pool: PgPool) {
    for (username, display_name, password, message) in [
        (" ", "A", PASSWORD, "請輸入帳號"),
        ("admin", "", PASSWORD, "請輸入顯示名稱"),
        ("admin", "A", "123456789", "密碼至少需要 10 個字元"),
    ] {
        let (status, body) = create_admin(&pool, username, display_name, password).await;
        let err = assert_error(status, &body, ErrorCode::Validation);
        assert_eq!(err.message, message);
    }
    assert_eq!(user_count(&pool).await, 0);
    assert!(status(&pool).await.needs_setup);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn existing_inactive_user_blocks_setup(pool: PgPool) {
    // 只要 users 有任何資料（即使已停用）就不允許
    TestUser::new("old", role::VIEWER)
        .inactive()
        .insert(&pool)
        .await
        .unwrap();
    assert!(!status(&pool).await.needs_setup);
    let (status, body) = create_admin(&pool, "admin", "A", PASSWORD).await;
    assert_error(status, &body, ErrorCode::Conflict);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn setup_routes_need_no_token(pool: PgPool) {
    // 帶了無效 token 也不影響（不需要登入的路由不檢查 token）
    let (status, body) = call(
        &pool,
        Method::GET,
        "/api/v1/setup/status",
        Some("invalid-token"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

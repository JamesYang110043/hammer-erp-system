//! 認證 extractor 與權限檢查的整合測試（#24）。
//!
//! 使用 `#[sqlx::test]`，需要 `DATABASE_URL`（見 README「整合測試」）。
//! 測試路由只在這裡定義，模擬之後各 API 的寫法。

#![allow(clippy::unwrap_used)]

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::routing::get;
use axum::{Json, Router};
use hammer_core::test_util::{TestUser, create_session, create_user_with_session};
use hammer_server::{AppError, AppState, AuthUser, AuthUserAllowPasswordChange, Config, router};
use hammer_shared::{ApiError, ErrorCode, Id, permission, role};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;

fn state(pool: PgPool) -> AppState {
    let config = Config::from_sources(None, |name| match name {
        "DATABASE_URL" => Some("postgres://unused".to_owned()),
        _ => None,
    })
    .unwrap();
    AppState::new(pool, config)
}

/// 需要 `record.write` 的路由。
async fn write_route(auth: AuthUser) -> Result<Json<Value>, AppError> {
    auth.require(permission::RECORD_WRITE)?;
    Ok(Json(json!({ "user_id": auth.user_id, "role": auth.role })))
}

/// 只需要登入的路由。
async fn read_route(auth: AuthUser) -> Json<Value> {
    Json(json!({ "user_id": auth.user_id }))
}

/// 強制改密碼期間仍可呼叫的路由（模擬 `/auth/me`）。
async fn me_route(auth: AuthUserAllowPasswordChange) -> Json<Value> {
    Json(json!({
        "user_id": auth.user_id,
        "must_change_password": auth.must_change_password,
    }))
}

fn test_app(pool: PgPool) -> Router {
    Router::new()
        .route("/api/v1/test/write", get(write_route))
        .route("/api/v1/test/read", get(read_route))
        .route("/api/v1/test/me", get(me_route))
        .with_state(state(pool))
}

async fn call(app: Router, uri: &str, authorization: Option<&str>) -> (StatusCode, Value) {
    let mut req = Request::get(uri);
    if let Some(value) = authorization {
        req = req.header(header::AUTHORIZATION, value);
    }
    let res = app.oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
    let status = res.status();
    let body = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap())
}

async fn call_with_token(pool: &PgPool, uri: &str, token: &str) -> (StatusCode, Value) {
    call(
        test_app(pool.clone()),
        uri,
        Some(&format!("Bearer {token}")),
    )
    .await
}

fn assert_error(status: StatusCode, body: Value, expected: ErrorCode) {
    assert_eq!(status.as_u16(), expected.http_status(), "{body}");
    let err: ApiError = serde_json::from_value(body).unwrap();
    assert_eq!(err.code, expected);
    assert!(!err.message.is_empty());
}

async fn session_times(pool: &PgPool, token: &str) -> (OffsetDateTime, OffsetDateTime) {
    sqlx::query_as("SELECT last_seen_at, expires_at FROM sessions WHERE token_hash = $1")
        .bind(hammer_core::auth::hash_token(token))
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn set_last_seen(pool: &PgPool, token: &str, last_seen_at: OffsetDateTime, idle: Duration) {
    sqlx::query("UPDATE sessions SET last_seen_at = $2, expires_at = $3 WHERE token_hash = $1")
        .bind(hammer_core::auth::hash_token(token))
        .bind(last_seen_at)
        .bind(last_seen_at + idle)
        .execute(pool)
        .await
        .unwrap();
}

async fn editor(pool: &PgPool) -> (Id, String) {
    let (id, token) = create_user_with_session(pool, &TestUser::new("editor", role::EDITOR))
        .await
        .unwrap();
    (id, token.into_string())
}

// ---------------------------------------------------------------------------
// 401
// ---------------------------------------------------------------------------

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn missing_or_malformed_token_is_401(pool: PgPool) {
    for authorization in [
        None,
        Some(""),
        Some("Bearer"),
        Some("Basic abc"),
        Some("abc"),
    ] {
        let (status, body) = call(test_app(pool.clone()), "/api/v1/test/read", authorization).await;
        assert_error(status, body, ErrorCode::Unauthorized);
    }
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn unknown_token_is_401(pool: PgPool) {
    editor(&pool).await;
    let (status, body) = call_with_token(&pool, "/api/v1/test/read", "not-a-real-token").await;
    assert_error(status, body, ErrorCode::Unauthorized);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn revoked_session_is_401(pool: PgPool) {
    let (_, token) = editor(&pool).await;
    let (status, _) = call_with_token(&pool, "/api/v1/test/read", &token).await;
    assert_eq!(status, StatusCode::OK);

    sqlx::query("UPDATE sessions SET revoked_at = now()")
        .execute(&pool)
        .await
        .unwrap();
    let (status, body) = call_with_token(&pool, "/api/v1/test/read", &token).await;
    assert_error(status, body, ErrorCode::Unauthorized);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn idle_timeout_is_401(pool: PgPool) {
    let (_, token) = editor(&pool).await;
    // 預設閒置 480 分鐘：最後使用在 481 分鐘前
    let now = OffsetDateTime::now_utc();
    set_last_seen(
        &pool,
        &token,
        now - Duration::minutes(481),
        Duration::minutes(480),
    )
    .await;
    let (status, body) = call_with_token(&pool, "/api/v1/test/read", &token).await;
    assert_error(status, body, ErrorCode::Unauthorized);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn shortened_idle_setting_applies_immediately(pool: PgPool) {
    let (_, token) = editor(&pool).await;
    let now = OffsetDateTime::now_utc();
    set_last_seen(
        &pool,
        &token,
        now - Duration::minutes(30),
        Duration::minutes(480),
    )
    .await;
    let (status, _) = call_with_token(&pool, "/api/v1/test/read", &token).await;
    assert_eq!(status, StatusCode::OK);

    set_last_seen(
        &pool,
        &token,
        now - Duration::minutes(30),
        Duration::minutes(480),
    )
    .await;
    sqlx::query("UPDATE settings SET value = '20' WHERE key = 'security.session_idle_minutes'")
        .execute(&pool)
        .await
        .unwrap();
    let (status, body) = call_with_token(&pool, "/api/v1/test/read", &token).await;
    assert_error(status, body, ErrorCode::Unauthorized);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn disabled_user_with_valid_token_is_401(pool: PgPool) {
    let (user_id, token) = editor(&pool).await;
    let (status, _) = call_with_token(&pool, "/api/v1/test/read", &token).await;
    assert_eq!(status, StatusCode::OK);

    sqlx::query("UPDATE users SET is_active = false WHERE id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();
    for uri in ["/api/v1/test/read", "/api/v1/test/me"] {
        let (status, body) = call_with_token(&pool, uri, &token).await;
        assert_error(status, body, ErrorCode::Unauthorized);
    }
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn inactive_user_created_disabled_is_401(pool: PgPool) {
    let (_, token) = create_user_with_session(&pool, &TestUser::new("bob", role::ADMIN).inactive())
        .await
        .unwrap();
    let (status, body) = call_with_token(&pool, "/api/v1/test/read", token.as_str()).await;
    assert_error(status, body, ErrorCode::Unauthorized);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn deleted_user_is_401(pool: PgPool) {
    let (user_id, token) = editor(&pool).await;
    sqlx::query("UPDATE users SET deleted_at = now() WHERE id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();
    let (status, body) = call_with_token(&pool, "/api/v1/test/read", &token).await;
    assert_error(status, body, ErrorCode::Unauthorized);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn unauthenticated_is_401_before_permission_check(pool: PgPool) {
    let (status, body) = call(test_app(pool), "/api/v1/test/write", None).await;
    assert_error(status, body, ErrorCode::Unauthorized);
}

// ---------------------------------------------------------------------------
// 權限
// ---------------------------------------------------------------------------

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn viewer_is_forbidden_editor_and_admin_pass(pool: PgPool) {
    let (_, viewer) = create_user_with_session(&pool, &TestUser::new("viewer", role::VIEWER))
        .await
        .unwrap();
    let (status, body) = call_with_token(&pool, "/api/v1/test/write", viewer.as_str()).await;
    assert_error(status, body.clone(), ErrorCode::Forbidden);
    assert_eq!(body["details"]["permission"], permission::RECORD_WRITE);
    // 只需要登入的路由，viewer 可以通過
    let (status, _) = call_with_token(&pool, "/api/v1/test/read", viewer.as_str()).await;
    assert_eq!(status, StatusCode::OK);

    let (editor_id, editor) = editor(&pool).await;
    let (status, body) = call_with_token(&pool, "/api/v1/test/write", &editor).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["user_id"], editor_id.to_string());
    assert_eq!(body["role"], role::EDITOR);

    let (_, admin) = create_user_with_session(&pool, &TestUser::new("admin", role::ADMIN))
        .await
        .unwrap();
    let (status, _) = call_with_token(&pool, "/api/v1/test/write", admin.as_str()).await;
    assert_eq!(status, StatusCode::OK);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn permission_change_applies_immediately(pool: PgPool) {
    let (_, token) = editor(&pool).await;
    let (status, _) = call_with_token(&pool, "/api/v1/test/write", &token).await;
    assert_eq!(status, StatusCode::OK);

    // 權限不快取：移除 editor 的 record.write 後，下一個請求立即生效
    sqlx::query(
        "DELETE FROM role_permissions
         WHERE permission = 'record.write'
           AND role_id = (SELECT id FROM roles WHERE code = 'editor')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let (status, body) = call_with_token(&pool, "/api/v1/test/write", &token).await;
    assert_error(status, body, ErrorCode::Forbidden);
}

// ---------------------------------------------------------------------------
// 強制改密碼
// ---------------------------------------------------------------------------

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn must_change_password_only_allows_permitted_routes(pool: PgPool) {
    let (user_id, token) = create_user_with_session(
        &pool,
        &TestUser::new("newbie", role::ADMIN).must_change_password(),
    )
    .await
    .unwrap();

    for uri in ["/api/v1/test/read", "/api/v1/test/write"] {
        let (status, body) = call_with_token(&pool, uri, token.as_str()).await;
        assert_error(status, body, ErrorCode::PasswordChangeRequired);
    }
    let (status, body) = call_with_token(&pool, "/api/v1/test/me", token.as_str()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["user_id"], user_id.to_string());
    assert_eq!(body["must_change_password"], true);

    // 改完密碼後其他 API 恢復可用
    sqlx::query("UPDATE users SET must_change_password = false WHERE id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();
    let (status, _) = call_with_token(&pool, "/api/v1/test/write", token.as_str()).await;
    assert_eq!(status, StatusCode::OK);
}

// ---------------------------------------------------------------------------
// last_seen_at
// ---------------------------------------------------------------------------

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn last_seen_updates_only_after_one_minute(pool: PgPool) {
    let (_, token) = editor(&pool).await;
    let now = OffsetDateTime::now_utc();

    // 30 秒前使用過：不寫入
    let recent = now - Duration::seconds(30);
    set_last_seen(&pool, &token, recent, Duration::minutes(480)).await;
    let before = session_times(&pool, &token).await;
    let (status, _) = call_with_token(&pool, "/api/v1/test/read", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(session_times(&pool, &token).await, before);

    // 2 分鐘前使用過：更新 last_seen_at，並延長 expires_at
    let old = now - Duration::minutes(2);
    set_last_seen(&pool, &token, old, Duration::minutes(480)).await;
    let (status, _) = call_with_token(&pool, "/api/v1/test/read", &token).await;
    assert_eq!(status, StatusCode::OK);
    let (last_seen_at, expires_at) = session_times(&pool, &token).await;
    assert!(last_seen_at >= now, "{last_seen_at} < {now}");
    assert_eq!(expires_at - last_seen_at, Duration::minutes(480));
}

// ---------------------------------------------------------------------------
// 不需要登入的路由
// ---------------------------------------------------------------------------

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn health_does_not_require_login(pool: PgPool) {
    let app = router(state(pool.clone()));
    let req = Request::get("/api/v1/health").body(Body::empty()).unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // 帶無效 token 也不影響
    let req = Request::get("/api/v1/health")
        .header(header::AUTHORIZATION, "Bearer invalid")
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn test_util_creates_session_for_existing_user(pool: PgPool) {
    let user_id = TestUser::new("carol", role::VIEWER)
        .password("correct horse battery")
        .insert(&pool)
        .await
        .unwrap();
    let token = create_session(&pool, user_id).await.unwrap();
    let (status, body) = call_with_token(&pool, "/api/v1/test/read", token.as_str()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["user_id"], user_id.to_string());

    // 未知角色回傳錯誤
    assert!(TestUser::new("dave", "nope").insert(&pool).await.is_err());
}

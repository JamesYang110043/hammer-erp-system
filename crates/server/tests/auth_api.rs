//! 登入、登出、`/auth/me`、修改密碼 API 的整合測試（#25）。
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
use hammer_shared::{ApiError, ErrorCode, Id, audit, permission, role};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tower::ServiceExt;

const PASSWORD: &str = "correct horse battery";
const NEW_PASSWORD: &str = "new password 123";

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

async fn login(pool: &PgPool, username: &str, password: &str) -> (StatusCode, Value) {
    call(
        pool,
        Method::POST,
        "/api/v1/auth/login",
        None,
        Some(json!({ "username": username, "password": password })),
    )
    .await
}

/// 登入成功並回傳 token。
async fn login_ok(pool: &PgPool, username: &str, password: &str) -> LoginResponse {
    let (status, body) = login(pool, username, password).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn me(pool: &PgPool, token: &str) -> (StatusCode, Value) {
    call(pool, Method::GET, "/api/v1/auth/me", Some(token), None).await
}

async fn logout(pool: &PgPool, token: &str) -> (StatusCode, Value) {
    call(pool, Method::POST, "/api/v1/auth/logout", Some(token), None).await
}

async fn change_password(
    pool: &PgPool,
    token: &str,
    current: &str,
    new: &str,
) -> (StatusCode, Value) {
    call(
        pool,
        Method::POST,
        "/api/v1/auth/change-password",
        Some(token),
        Some(json!({ "current_password": current, "new_password": new })),
    )
    .await
}

fn assert_error(status: StatusCode, body: &Value, expected: ErrorCode) -> ApiError {
    assert_eq!(status.as_u16(), expected.http_status(), "{body}");
    let err: ApiError = serde_json::from_value(body.clone()).unwrap();
    assert_eq!(err.code, expected, "{body}");
    assert!(!err.message.is_empty());
    err
}

async fn user(pool: &PgPool, username: &str, role: &str) -> Id {
    TestUser::new(username, role)
        .password(PASSWORD)
        .insert(pool)
        .await
        .unwrap()
}

#[derive(Debug, sqlx::FromRow)]
struct LoginState {
    failed_login_count: i32,
    locked_until: Option<OffsetDateTime>,
    last_login_at: Option<OffsetDateTime>,
    must_change_password: bool,
    version: i32,
}

async fn login_state(pool: &PgPool, id: Id) -> LoginState {
    sqlx::query_as(
        "SELECT failed_login_count, locked_until, last_login_at, must_change_password, version
         FROM users WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap()
}

// ---------------------------------------------------------------------------
// 登入
// ---------------------------------------------------------------------------

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn login_returns_token_usable_for_me(pool: PgPool) {
    let id = user(&pool, "alice", role::EDITOR).await;
    let before = OffsetDateTime::now_utc();

    let res = login_ok(&pool, "alice", PASSWORD).await;
    assert_eq!(res.token.len(), 43);
    assert_eq!(res.user.id, id);
    assert_eq!(res.user.username, "alice");
    assert_eq!(res.user.display_name, "alice");
    assert_eq!(res.user.role, role::EDITOR);
    assert!(!res.user.must_change_password);
    let mut expected = vec![
        permission::DATA_EXPORT,
        permission::RECORD_POST,
        permission::RECORD_READ,
        permission::RECORD_WRITE,
    ];
    expected.sort_unstable();
    assert_eq!(res.user.permissions, expected);

    let (status, body) = me(&pool, &res.token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let current: CurrentUser = serde_json::from_value(body).unwrap();
    assert_eq!(current, res.user);

    let state = login_state(&pool, id).await;
    assert!(state.last_login_at.unwrap() >= before);
    assert_eq!(state.failed_login_count, 0);
    // 登入不遞增 version
    assert_eq!(state.version, 1);

    // session 記錄 client_info
    let info: Option<String> =
        sqlx::query_scalar("SELECT client_info FROM sessions WHERE user_id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(info.as_deref(), Some("HammerERP-test / 0.1.0"));
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn unknown_user_and_wrong_password_are_identical(pool: PgPool) {
    user(&pool, "alice", role::VIEWER).await;

    let unknown = login(&pool, "nobody", PASSWORD).await;
    let wrong = login(&pool, "alice", "wrong password!!").await;
    let err = assert_error(unknown.0, &unknown.1, ErrorCode::Unauthorized);
    assert_eq!(err.message, "帳號或密碼錯誤");
    assert_eq!(err.details, None);
    assert_eq!(unknown, wrong);

    // 已刪除的帳號視同不存在
    sqlx::query("UPDATE users SET deleted_at = now() WHERE username = 'alice'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(login(&pool, "alice", PASSWORD).await, unknown);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn locks_after_five_failures_and_unlocks_after_locked_until(pool: PgPool) {
    let id = user(&pool, "alice", role::EDITOR).await;

    for i in 1..=5 {
        let (status, body) = login(&pool, "alice", "wrong password!!").await;
        assert_error(status, &body, ErrorCode::Unauthorized);
        let state = login_state(&pool, id).await;
        if i < 5 {
            assert_eq!(state.failed_login_count, i);
            assert_eq!(state.locked_until, None);
        } else {
            assert!(state.locked_until.unwrap() > OffsetDateTime::now_utc());
        }
    }

    // 鎖定中：正確密碼也無法登入
    let (status, body) = login(&pool, "alice", PASSWORD).await;
    let err = assert_error(status, &body, ErrorCode::AccountLocked);
    let locked_until = login_state(&pool, id).await.locked_until.unwrap();
    let details = err.details.unwrap();
    let reported =
        OffsetDateTime::parse(details["locked_until"].as_str().unwrap(), &Rfc3339).unwrap();
    assert_eq!(reported.unix_timestamp(), locked_until.unix_timestamp());

    // 把 locked_until 調到過去後可以登入，失敗次數歸零
    sqlx::query("UPDATE users SET locked_until = now() - interval '1 second' WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    login_ok(&pool, "alice", PASSWORD).await;
    let state = login_state(&pool, id).await;
    assert_eq!(state.failed_login_count, 0);
    assert_eq!(state.locked_until, None);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn success_resets_failure_count(pool: PgPool) {
    let id = user(&pool, "alice", role::EDITOR).await;
    for _ in 0..4 {
        login(&pool, "alice", "wrong password!!").await;
    }
    assert_eq!(login_state(&pool, id).await.failed_login_count, 4);
    login_ok(&pool, "alice", PASSWORD).await;
    assert_eq!(login_state(&pool, id).await.failed_login_count, 0);
    // 再錯一次不會鎖定
    login(&pool, "alice", "wrong password!!").await;
    assert_eq!(login_state(&pool, id).await.locked_until, None);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn disabled_account(pool: PgPool) {
    TestUser::new("alice", role::EDITOR)
        .password(PASSWORD)
        .inactive()
        .insert(&pool)
        .await
        .unwrap();
    let (status, body) = login(&pool, "alice", PASSWORD).await;
    assert_error(status, &body, ErrorCode::AccountDisabled);
    // 密碼錯誤時不透露帳號已停用
    let (status, body) = login(&pool, "alice", "wrong password!!").await;
    assert_error(status, &body, ErrorCode::Unauthorized);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn must_change_password_user_can_login_and_call_me(pool: PgPool) {
    TestUser::new("newbie", role::VIEWER)
        .password(PASSWORD)
        .must_change_password()
        .insert(&pool)
        .await
        .unwrap();
    let res = login_ok(&pool, "newbie", PASSWORD).await;
    assert!(res.user.must_change_password);
    let (status, body) = me(&pool, &res.token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["must_change_password"], true);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn malformed_login_body_is_rejected(pool: PgPool) {
    let (status, body) = call(
        &pool,
        Method::POST,
        "/api/v1/auth/login",
        None,
        Some(json!({ "username": "alice" })),
    )
    .await;
    assert!(status.is_client_error(), "{status}");
    let _: ApiError = serde_json::from_value(body).unwrap();
}

// ---------------------------------------------------------------------------
// 登出
// ---------------------------------------------------------------------------

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn logout_revokes_token(pool: PgPool) {
    user(&pool, "alice", role::VIEWER).await;
    let a = login_ok(&pool, "alice", PASSWORD).await;
    let b = login_ok(&pool, "alice", PASSWORD).await;

    let (status, body) = logout(&pool, &a.token).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let (status, body) = me(&pool, &a.token).await;
    assert_error(status, &body, ErrorCode::Unauthorized);
    let (status, body) = logout(&pool, &a.token).await;
    assert_error(status, &body, ErrorCode::Unauthorized);

    // 只撤銷目前的 session
    let (status, _) = me(&pool, &b.token).await;
    assert_eq!(status, StatusCode::OK);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn protected_auth_routes_require_token(pool: PgPool) {
    for (method, uri) in [
        (Method::GET, "/api/v1/auth/me"),
        (Method::POST, "/api/v1/auth/logout"),
    ] {
        let (status, body) = call(&pool, method, uri, None, None).await;
        assert_error(status, &body, ErrorCode::Unauthorized);
    }
    let (status, body) = call(
        &pool,
        Method::POST,
        "/api/v1/auth/change-password",
        None,
        Some(json!({ "current_password": PASSWORD, "new_password": NEW_PASSWORD })),
    )
    .await;
    assert_error(status, &body, ErrorCode::Unauthorized);
}

// ---------------------------------------------------------------------------
// 修改密碼
// ---------------------------------------------------------------------------

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn change_password_revokes_other_sessions(pool: PgPool) {
    let id = user(&pool, "alice", role::EDITOR).await;
    let current = login_ok(&pool, "alice", PASSWORD).await;
    let other = login_ok(&pool, "alice", PASSWORD).await;

    let (status, body) = change_password(&pool, &current.token, PASSWORD, NEW_PASSWORD).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    // 舊密碼失效，新密碼可以登入
    let (status, body) = login(&pool, "alice", PASSWORD).await;
    assert_error(status, &body, ErrorCode::Unauthorized);
    login_ok(&pool, "alice", NEW_PASSWORD).await;

    // 其他 session 被撤銷，目前的 session 仍有效
    let (status, body) = me(&pool, &other.token).await;
    assert_error(status, &body, ErrorCode::Unauthorized);
    let (status, _) = me(&pool, &current.token).await;
    assert_eq!(status, StatusCode::OK);

    // 修改密碼會遞增 version
    assert_eq!(login_state(&pool, id).await.version, 2);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn change_password_validation(pool: PgPool) {
    user(&pool, "alice", role::EDITOR).await;
    let token = login_ok(&pool, "alice", PASSWORD).await.token;

    let cases = [
        ("wrong password!!", NEW_PASSWORD, "目前的密碼不正確"),
        (PASSWORD, "short", "密碼至少需要 10 個字元"),
        (PASSWORD, PASSWORD, "新密碼不可與目前的密碼相同"),
    ];
    for (current, new, message) in cases {
        let (status, body) = change_password(&pool, &token, current, new).await;
        let err = assert_error(status, &body, ErrorCode::Validation);
        assert_eq!(err.message, message);
    }
    // 都沒有改到：原密碼仍可登入
    login_ok(&pool, "alice", PASSWORD).await;
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn change_password_clears_must_change_password(pool: PgPool) {
    let id = TestUser::new("newbie", role::EDITOR)
        .password(PASSWORD)
        .must_change_password()
        .insert(&pool)
        .await
        .unwrap();
    let token = login_ok(&pool, "newbie", PASSWORD).await.token;

    let (status, body) = change_password(&pool, &token, PASSWORD, NEW_PASSWORD).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert!(!login_state(&pool, id).await.must_change_password);
    let (_, body) = me(&pool, &token).await;
    assert_eq!(body["must_change_password"], false);
}

// ---------------------------------------------------------------------------
// 操作紀錄
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

async fn audit_rows(pool: &PgPool) -> Vec<AuditRow> {
    sqlx::query_as(
        "SELECT user_id, action, entity_key, record_id, before_data, after_data, client_info
         FROM audit_logs ORDER BY id",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn audit_logs_are_written(pool: PgPool) {
    let id = user(&pool, "alice", role::EDITOR).await;

    login(&pool, "nobody", PASSWORD).await;
    login(&pool, "alice", "wrong password!!").await;
    let token = login_ok(&pool, "alice", PASSWORD).await.token;
    change_password(&pool, &token, PASSWORD, NEW_PASSWORD).await;
    logout(&pool, &token).await;

    let rows = audit_rows(&pool).await;
    let actions: Vec<&str> = rows.iter().map(|r| r.action.as_str()).collect();
    assert_eq!(
        actions,
        vec![
            audit::LOGIN_FAILED,
            audit::LOGIN_FAILED,
            audit::LOGIN,
            audit::PASSWORD_CHANGE,
            audit::LOGOUT,
        ]
    );

    // 帳號不存在：沒有 user_id，記錄嘗試的帳號
    assert_eq!(rows[0].user_id, None);
    assert_eq!(
        rows[0].after_data,
        Some(json!({ "username": "nobody", "reason": "unknown_user" }))
    );
    // 密碼錯誤：有 user_id
    assert_eq!(rows[1].user_id, Some(id));
    assert_eq!(
        rows[1].after_data,
        Some(json!({ "username": "alice", "reason": "wrong_password" }))
    );
    for row in &rows[2..] {
        assert_eq!(row.user_id, Some(id));
    }
    // 修改密碼：對應到使用者資料，不記錄密碼內容
    assert_eq!(rows[3].entity_key.as_deref(), Some("user"));
    assert_eq!(rows[3].record_id, Some(id));
    assert_eq!(rows[3].before_data, None);
    assert_eq!(rows[3].after_data, None);

    for row in &rows {
        assert_eq!(row.client_info.as_deref(), Some("HammerERP-test / 0.1.0"));
        let text = format!("{:?} {:?}", row.before_data, row.after_data);
        assert!(
            !text.contains(PASSWORD) && !text.contains(NEW_PASSWORD),
            "{text}"
        );
        assert!(!text.contains("argon2"), "{text}");
    }
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn locked_and_disabled_attempts_are_audited(pool: PgPool) {
    let id = user(&pool, "alice", role::EDITOR).await;
    sqlx::query("UPDATE users SET locked_until = now() + interval '10 minutes' WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    login(&pool, "alice", PASSWORD).await;

    sqlx::query("UPDATE users SET locked_until = NULL, is_active = false WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    login(&pool, "alice", PASSWORD).await;

    let reasons: Vec<Value> = audit_rows(&pool)
        .await
        .into_iter()
        .map(|r| {
            assert_eq!(r.action, audit::LOGIN_FAILED);
            r.after_data.unwrap()["reason"].clone()
        })
        .collect();
    assert_eq!(reasons, vec![json!("locked"), json!("disabled")]);
}

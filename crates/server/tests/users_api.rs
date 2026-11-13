//! 帳號管理 API 的整合測試（#27）。
//!
//! 使用 `#[sqlx::test]`，需要 `DATABASE_URL`（見 README「整合測試」）。
//! 透過完整的 router 呼叫，包含所有中介層。

#![allow(clippy::unwrap_used)]

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use hammer_core::test_util::{TestUser, create_user_with_session};
use hammer_server::{AppState, Config, router};
use hammer_shared::auth::LoginResponse;
use hammer_shared::user::{Role, User};
use hammer_shared::{ApiError, ErrorCode, Id, audit, permission, role};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

const PASSWORD: &str = "correct horse battery";
const NEW_PASSWORD: &str = "temporary password 1";

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

fn assert_error(status: StatusCode, body: &Value, expected: ErrorCode) -> ApiError {
    assert_eq!(status.as_u16(), expected.http_status(), "{body}");
    let err: ApiError = serde_json::from_value(body.clone()).unwrap();
    assert_eq!(err.code, expected, "{body}");
    assert!(!err.message.is_empty());
    err
}

fn assert_last_admin(status: StatusCode, body: &Value) {
    let err = assert_error(status, body, ErrorCode::Validation);
    assert!(err.message.contains("管理者"), "{body}");
    assert_eq!(err.details, Some(json!({ "reason": "last_admin" })));
}

/// 建立可登入的使用者並取得 token。
async fn user_with_token(pool: &PgPool, username: &str, role: &str) -> (Id, String) {
    let (id, token) =
        create_user_with_session(pool, &TestUser::new(username, role).password(PASSWORD))
            .await
            .unwrap();
    (id, token.into_string())
}

async fn admin(pool: &PgPool) -> (Id, String) {
    user_with_token(pool, "admin", role::ADMIN).await
}

async fn get_user(pool: &PgPool, token: &str, id: Id) -> User {
    let (status, body) = call(
        pool,
        Method::GET,
        &format!("/api/v1/users/{id}"),
        Some(token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn create_user(
    pool: &PgPool,
    token: &str,
    username: &str,
    role: &str,
) -> (StatusCode, Value) {
    call(
        pool,
        Method::POST,
        "/api/v1/users",
        Some(token),
        Some(json!({
            "username": username,
            "display_name": format!("{username} 名稱"),
            "role": role,
            "password": PASSWORD,
        })),
    )
    .await
}

async fn create_user_ok(pool: &PgPool, token: &str, username: &str, role: &str) -> User {
    let (status, body) = create_user(pool, token, username, role).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn patch(pool: &PgPool, token: &str, id: Id, body: Value) -> (StatusCode, Value) {
    call(
        pool,
        Method::PATCH,
        &format!("/api/v1/users/{id}"),
        Some(token),
        Some(body),
    )
    .await
}

async fn delete(pool: &PgPool, token: &str, id: Id, version: i32) -> (StatusCode, Value) {
    call(
        pool,
        Method::DELETE,
        &format!("/api/v1/users/{id}?version={version}"),
        Some(token),
        None,
    )
    .await
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

/// 用 token 呼叫 `/auth/me`，回傳狀態碼。
async fn me_status(pool: &PgPool, token: &str) -> StatusCode {
    call(pool, Method::GET, "/api/v1/auth/me", Some(token), None)
        .await
        .0
}

#[derive(Debug, sqlx::FromRow)]
struct AuditRow {
    user_id: Option<Id>,
    action: String,
    entity_key: Option<String>,
    record_id: Option<Id>,
    before_data: Option<Value>,
    after_data: Option<Value>,
}

async fn audit_logs(pool: &PgPool, record_id: Id) -> Vec<AuditRow> {
    sqlx::query_as(
        "SELECT user_id, action, entity_key, record_id, before_data, after_data
         FROM audit_logs WHERE record_id = $1 ORDER BY occurred_at, id",
    )
    .bind(record_id)
    .fetch_all(pool)
    .await
    .unwrap()
}

// ---------------------------------------------------------------------------
// 驗收條件
// ---------------------------------------------------------------------------

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn editor_and_viewer_are_forbidden(pool: PgPool) {
    let (admin_id, _) = admin(&pool).await;
    for (username, role_code) in [("editor", role::EDITOR), ("viewer", role::VIEWER)] {
        let (_, token) = user_with_token(&pool, username, role_code).await;
        let user = format!("/api/v1/users/{admin_id}");
        let requests: Vec<(Method, String, Option<Value>)> = vec![
            (Method::GET, "/api/v1/roles".into(), None),
            (Method::GET, "/api/v1/users".into(), None),
            (Method::GET, user.clone(), None),
            (
                Method::POST,
                "/api/v1/users".into(),
                Some(
                    json!({ "username": "x", "display_name": "X", "role": role::ADMIN, "password": PASSWORD }),
                ),
            ),
            (
                Method::PATCH,
                user.clone(),
                Some(json!({ "version": 1, "is_active": false })),
            ),
            (
                Method::POST,
                format!("{user}/reset-password"),
                Some(json!({ "new_password": NEW_PASSWORD })),
            ),
            (Method::POST, format!("{user}/unlock"), None),
            (Method::DELETE, format!("{user}?version=1"), None),
        ];
        for (method, uri, body) in requests {
            let (status, res) = call(&pool, method.clone(), &uri, Some(&token), body).await;
            let err = assert_error(status, &res, ErrorCode::Forbidden);
            assert_eq!(
                err.details,
                Some(json!({ "permission": permission::USER_MANAGE })),
                "{role_code} {method} {uri}"
            );
        }
    }
    // 沒有任何資料被改變
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_logs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let (version, active): (i32, bool) =
        sqlx::query_as("SELECT version, is_active FROM users WHERE id = $1")
            .bind(admin_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((version, active), (1, true));
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn created_user_must_change_password(pool: PgPool) {
    let (_, token) = admin(&pool).await;
    let user = create_user_ok(&pool, &token, "bob", role::EDITOR).await;
    assert_eq!(user.username, "bob");
    assert_eq!(user.display_name, "bob 名稱");
    assert_eq!(user.role, role::EDITOR);
    assert_eq!(user.role_name, "可編輯者");
    assert!(user.is_active && !user.is_locked && user.must_change_password);
    assert_eq!(user.last_login_at, None);
    assert_eq!(user.version, 1);

    let (status, body) = login(&pool, "bob", PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let res: LoginResponse = serde_json::from_value(body).unwrap();
    assert!(res.user.must_change_password);
    assert_eq!(res.user.role, role::EDITOR);

    // 登入後列表會顯示最後登入時間
    assert!(
        get_user(&pool, &token, user.id)
            .await
            .last_login_at
            .is_some()
    );
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn sole_admin_cannot_remove_self(pool: PgPool) {
    let (id, token) = admin(&pool).await;
    // 其他角色的帳號不算管理者
    create_user_ok(&pool, &token, "editor", role::EDITOR).await;

    let (status, body) = patch(
        &pool,
        &token,
        id,
        json!({ "version": 1, "is_active": false }),
    )
    .await;
    assert_last_admin(status, &body);
    for other in [role::EDITOR, role::VIEWER] {
        let (status, body) = patch(&pool, &token, id, json!({ "version": 1, "role": other })).await;
        assert_last_admin(status, &body);
    }
    let (status, body) = delete(&pool, &token, id, 1).await;
    assert_last_admin(status, &body);

    // 全部 rollback：資料、session、操作紀錄都沒有變動
    let me = get_user(&pool, &token, id).await;
    assert!(me.is_active);
    assert_eq!(me.role, role::ADMIN);
    assert_eq!(me.version, 1);
    assert_eq!(me_status(&pool, &token).await, StatusCode::OK);
    assert!(audit_logs(&pool, id).await.is_empty());

    // 只改顯示名稱可以
    let (status, body) = patch(
        &pool,
        &token,
        id,
        json!({ "version": 1, "display_name": "老闆" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn one_of_two_admins_can_be_deactivated(pool: PgPool) {
    let (a_id, a_token) = admin(&pool).await;
    let (b_id, b_token) = user_with_token(&pool, "admin2", role::ADMIN).await;

    let (status, body) = patch(
        &pool,
        &a_token,
        b_id,
        json!({ "version": 1, "is_active": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let b: User = serde_json::from_value(body).unwrap();
    assert!(!b.is_active);
    assert_eq!(b.version, 2);
    // 被停用的管理者立即被登出
    assert_eq!(me_status(&pool, &b_token).await, StatusCode::UNAUTHORIZED);

    // 現在 a 是唯一有效的管理者
    let a = get_user(&pool, &a_token, a_id).await;
    let (status, body) = patch(
        &pool,
        &a_token,
        a_id,
        json!({ "version": a.version, "is_active": false }),
    )
    .await;
    assert_last_admin(status, &body);

    // 重新啟用 b 之後，a 可以停用自己
    let (status, body) = patch(
        &pool,
        &a_token,
        b_id,
        json!({ "version": 2, "is_active": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = patch(
        &pool,
        &a_token,
        a_id,
        json!({ "version": a.version, "is_active": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(me_status(&pool, &a_token).await, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn reset_password_revokes_tokens(pool: PgPool) {
    let (_, admin_token) = admin(&pool).await;
    let (id, token) = user_with_token(&pool, "bob", role::EDITOR).await;
    // 讓帳號處於鎖定狀態
    sqlx::query("UPDATE users SET failed_login_count = 5, locked_until = now() + interval '10 minutes' WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(me_status(&pool, &token).await, StatusCode::OK);

    let (status, body) = call(
        &pool,
        Method::POST,
        &format!("/api/v1/users/{id}/reset-password"),
        Some(&admin_token),
        Some(json!({ "new_password": NEW_PASSWORD })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let user: User = serde_json::from_value(body).unwrap();
    assert!(user.must_change_password);
    assert!(!user.is_locked);
    assert_eq!(user.locked_until, None);
    assert_eq!(user.version, 2);

    // 原本的 token 失效
    let (status, body) = call(&pool, Method::GET, "/api/v1/auth/me", Some(&token), None).await;
    assert_error(status, &body, ErrorCode::Unauthorized);

    // 舊密碼不能用，新密碼可以登入且必須修改密碼
    let (status, body) = login(&pool, "bob", PASSWORD).await;
    assert_error(status, &body, ErrorCode::Unauthorized);
    let (status, body) = login(&pool, "bob", NEW_PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let res: LoginResponse = serde_json::from_value(body).unwrap();
    assert!(res.user.must_change_password);

    // 操作紀錄不含密碼
    let logs = audit_logs(&pool, id).await;
    let log = logs
        .iter()
        .find(|l| l.action == audit::PASSWORD_CHANGE)
        .unwrap();
    let text = format!("{:?} {:?}", log.before_data, log.after_data);
    assert!(
        !text.contains(NEW_PASSWORD) && !text.contains("argon2"),
        "{text}"
    );
    assert_eq!(
        log.after_data.as_ref().unwrap()["must_change_password"],
        json!(true)
    );
    assert_eq!(
        log.after_data.as_ref().unwrap()["locked_until"],
        Value::Null
    );
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn version_mismatch_is_conflict(pool: PgPool) {
    let (_, token) = admin(&pool).await;
    let user = create_user_ok(&pool, &token, "bob", role::VIEWER).await;

    let (status, body) = patch(
        &pool,
        &token,
        user.id,
        json!({ "version": 1, "display_name": "Bob" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // 用舊的版本號修改或刪除
    let (status, body) = patch(
        &pool,
        &token,
        user.id,
        json!({ "version": 1, "display_name": "Robert" }),
    )
    .await;
    let err = assert_error(status, &body, ErrorCode::Conflict);
    assert_eq!(err.message, "此資料已被其他人修改，請重新載入");
    let (status, body) = patch(&pool, &token, user.id, json!({ "version": 1 })).await;
    assert_error(status, &body, ErrorCode::Conflict);
    let (status, body) = delete(&pool, &token, user.id, 1).await;
    assert_error(status, &body, ErrorCode::Conflict);

    let current = get_user(&pool, &token, user.id).await;
    assert_eq!(current.display_name, "Bob");
    assert_eq!(current.version, 2);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn duplicate_username_reports_field(pool: PgPool) {
    let (_, token) = admin(&pool).await;
    let bob = create_user_ok(&pool, &token, "bob", role::VIEWER).await;

    for username in ["bob", " bob ", "admin"] {
        let (status, body) = create_user(&pool, &token, username, role::EDITOR).await;
        let err = assert_error(status, &body, ErrorCode::Validation);
        assert_eq!(err.message, "此帳號已被使用");
        assert_eq!(err.details, Some(json!({ "field": "username" })));
    }

    // 已刪除的帳號名稱也不能再使用
    let (status, body) = delete(&pool, &token, bob.id, 1).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (status, body) = create_user(&pool, &token, "bob", role::EDITOR).await;
    let err = assert_error(status, &body, ErrorCode::Validation);
    assert_eq!(err.details, Some(json!({ "field": "username" })));
}

// ---------------------------------------------------------------------------
// 其他行為
// ---------------------------------------------------------------------------

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn invalid_input_reports_field(pool: PgPool) {
    let (id, token) = admin(&pool).await;
    for (body, field) in [
        (
            json!({ "username": " ", "display_name": "A", "role": role::VIEWER, "password": PASSWORD }),
            "username",
        ),
        (
            json!({ "username": "x", "display_name": "", "role": role::VIEWER, "password": PASSWORD }),
            "display_name",
        ),
        (
            json!({ "username": "x", "display_name": "A", "role": "root", "password": PASSWORD }),
            "role",
        ),
        (
            json!({ "username": "x", "display_name": "A", "role": role::VIEWER, "password": "short" }),
            "password",
        ),
    ] {
        let (status, res) = call(
            &pool,
            Method::POST,
            "/api/v1/users",
            Some(&token),
            Some(body),
        )
        .await;
        let err = assert_error(status, &res, ErrorCode::Validation);
        assert_eq!(err.details, Some(json!({ "field": field })), "{res}");
    }

    let (status, res) = patch(
        &pool,
        &token,
        id,
        json!({ "version": 1, "display_name": "  " }),
    )
    .await;
    let err = assert_error(status, &res, ErrorCode::Validation);
    assert_eq!(err.details, Some(json!({ "field": "display_name" })));
    let (status, res) = patch(&pool, &token, id, json!({ "version": 1, "role": "root" })).await;
    let err = assert_error(status, &res, ErrorCode::Validation);
    assert_eq!(err.details, Some(json!({ "field": "role" })));

    let (status, res) = call(
        &pool,
        Method::POST,
        &format!("/api/v1/users/{id}/reset-password"),
        Some(&token),
        Some(json!({ "new_password": "short" })),
    )
    .await;
    let err = assert_error(status, &res, ErrorCode::Validation);
    assert_eq!(err.details, Some(json!({ "field": "new_password" })));
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn role_change_and_delete_revoke_sessions(pool: PgPool) {
    let (_, admin_token) = admin(&pool).await;
    let (bob, bob_token) = user_with_token(&pool, "bob", role::EDITOR).await;
    let (carol, carol_token) = user_with_token(&pool, "carol", role::EDITOR).await;
    let (dave, dave_token) = user_with_token(&pool, "dave", role::EDITOR).await;

    // 只改顯示名稱不撤銷
    let (status, _) = patch(
        &pool,
        &admin_token,
        bob,
        json!({ "version": 1, "display_name": "Bobby" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me_status(&pool, &bob_token).await, StatusCode::OK);

    // 變更角色撤銷
    let (status, body) = patch(
        &pool,
        &admin_token,
        bob,
        json!({ "version": 2, "role": role::VIEWER }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(me_status(&pool, &bob_token).await, StatusCode::UNAUTHORIZED);
    // 重新登入後是新角色
    let (_, body) = login(&pool, "bob", PASSWORD).await;
    let res: LoginResponse = serde_json::from_value(body).unwrap();
    assert_eq!(res.user.role, role::VIEWER);

    // 停用撤銷，且無法登入
    let (status, _) = patch(
        &pool,
        &admin_token,
        carol,
        json!({ "version": 1, "is_active": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        me_status(&pool, &carol_token).await,
        StatusCode::UNAUTHORIZED
    );
    let (status, body) = login(&pool, "carol", PASSWORD).await;
    assert_error(status, &body, ErrorCode::AccountDisabled);

    // 刪除撤銷，且無法登入、查不到
    let (status, _) = delete(&pool, &admin_token, dave, 1).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        me_status(&pool, &dave_token).await,
        StatusCode::UNAUTHORIZED
    );
    let (status, body) = login(&pool, "dave", PASSWORD).await;
    assert_error(status, &body, ErrorCode::Unauthorized);
    let (status, body) = call(
        &pool,
        Method::GET,
        &format!("/api/v1/users/{dave}"),
        Some(&admin_token),
        None,
    )
    .await;
    assert_error(status, &body, ErrorCode::NotFound);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn list_users_and_roles(pool: PgPool) {
    let (_, token) = admin(&pool).await;
    let carol = create_user_ok(&pool, &token, "carol", role::VIEWER).await;
    create_user_ok(&pool, &token, "bob", role::EDITOR).await;
    delete(&pool, &token, carol.id, 1).await;

    let (status, body) = call(&pool, Method::GET, "/api/v1/users", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let users: Vec<User> = serde_json::from_value(body).unwrap();
    let names: Vec<&str> = users.iter().map(|u| u.username.as_str()).collect();
    assert_eq!(names, ["admin", "bob"], "已刪除的不列出，依帳號排序");

    let (status, body) = call(&pool, Method::GET, "/api/v1/roles", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let roles: Vec<Role> = serde_json::from_value(body).unwrap();
    let codes: Vec<&str> = roles.iter().map(|r| r.code.as_str()).collect();
    assert_eq!(codes, [role::ADMIN, role::EDITOR, role::VIEWER]);
    assert_eq!(roles[0].name, "管理者");
    let mut all: Vec<&str> = permission::ALL.to_vec();
    all.sort_unstable();
    assert_eq!(roles[0].permissions, all);
    assert_eq!(
        roles[2].permissions,
        [permission::DATA_EXPORT, permission::RECORD_READ]
    );
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn unlock_clears_lock(pool: PgPool) {
    let (_, token) = admin(&pool).await;
    let (id, _) = user_with_token(&pool, "bob", role::EDITOR).await;
    for _ in 0..5 {
        login(&pool, "bob", "wrong password!").await;
    }
    let (status, body) = login(&pool, "bob", PASSWORD).await;
    assert_error(status, &body, ErrorCode::AccountLocked);
    let user = get_user(&pool, &token, id).await;
    assert!(user.is_locked && user.locked_until.is_some());

    let (status, body) = call(
        &pool,
        Method::POST,
        &format!("/api/v1/users/{id}/unlock"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let unlocked: User = serde_json::from_value(body).unwrap();
    assert!(!unlocked.is_locked);
    assert_eq!(unlocked.locked_until, None);
    assert_eq!(unlocked.version, user.version, "解除鎖定不遞增 version");

    let (status, body) = login(&pool, "bob", PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let logs = audit_logs(&pool, id).await;
    let unlock_logs: Vec<_> = logs.iter().filter(|l| l.action == audit::UPDATE).collect();
    assert_eq!(unlock_logs.len(), 1);
    // 鎖定時失敗次數已歸零，只有 locked_until 改變
    assert_eq!(
        unlock_logs[0].after_data,
        Some(json!({ "locked_until": null }))
    );
    assert!(unlock_logs[0].before_data.as_ref().unwrap()["locked_until"].is_string());

    // 沒有鎖定時再解除一次，不寫入紀錄
    let (status, _) = call(
        &pool,
        Method::POST,
        &format!("/api/v1/users/{id}/unlock"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        audit_logs(&pool, id)
            .await
            .iter()
            .filter(|l| l.action == audit::UPDATE)
            .count(),
        1
    );
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn writes_audit_logs_without_password(pool: PgPool) {
    let (admin_id, token) = admin(&pool).await;
    let user = create_user_ok(&pool, &token, "bob", role::VIEWER).await;
    let (status, _) = patch(
        &pool,
        &token,
        user.id,
        json!({ "version": 1, "role": role::EDITOR, "display_name": "Bob" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = delete(&pool, &token, user.id, 2).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let logs = audit_logs(&pool, user.id).await;
    let actions: Vec<&str> = logs.iter().map(|l| l.action.as_str()).collect();
    assert_eq!(actions, [audit::CREATE, audit::UPDATE, audit::DELETE]);
    for log in &logs {
        assert_eq!(log.user_id, Some(admin_id));
        assert_eq!(log.entity_key.as_deref(), Some("user"));
        assert_eq!(log.record_id, Some(user.id));
        let text = format!("{:?} {:?}", log.before_data, log.after_data);
        assert!(
            !text.contains(PASSWORD) && !text.contains("argon2"),
            "{text}"
        );
    }
    assert_eq!(logs[0].before_data, None);
    assert_eq!(
        logs[0].after_data,
        Some(json!({
            "username": "bob",
            "display_name": "bob 名稱",
            "role": role::VIEWER,
            "is_active": true,
            "must_change_password": true,
        }))
    );
    assert_eq!(
        logs[1].before_data,
        Some(json!({ "display_name": "bob 名稱", "role": role::VIEWER }))
    );
    assert_eq!(
        logs[1].after_data,
        Some(json!({ "display_name": "Bob", "role": role::EDITOR }))
    );
    assert_eq!(
        logs[2].before_data,
        Some(
            json!({ "username": "bob", "display_name": "Bob", "role": role::EDITOR, "is_active": true })
        )
    );
    assert_eq!(logs[2].after_data, None);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn unchanged_patch_writes_nothing(pool: PgPool) {
    let (id, token) = admin(&pool).await;
    let (status, body) = patch(
        &pool,
        &token,
        id,
        json!({ "version": 1, "role": role::ADMIN, "is_active": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let user: User = serde_json::from_value(body).unwrap();
    assert_eq!(user.version, 1);
    assert!(audit_logs(&pool, id).await.is_empty());
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn missing_user_is_not_found(pool: PgPool) {
    let (_, token) = admin(&pool).await;
    let missing = hammer_shared::new_id();
    let user = format!("/api/v1/users/{missing}");
    for (method, uri, body) in [
        (Method::GET, user.clone(), None),
        (
            Method::PATCH,
            user.clone(),
            Some(json!({ "version": 1, "display_name": "X" })),
        ),
        (
            Method::POST,
            format!("{user}/reset-password"),
            Some(json!({ "new_password": NEW_PASSWORD })),
        ),
        (Method::POST, format!("{user}/unlock"), None),
        (Method::DELETE, format!("{user}?version=1"), None),
    ] {
        let (status, res) = call(&pool, method, &uri, Some(&token), body).await;
        let err = assert_error(status, &res, ErrorCode::NotFound);
        assert_eq!(err.message, "找不到此帳號");
    }

    // id 格式錯誤、缺少 version
    let (status, res) = call(
        &pool,
        Method::GET,
        "/api/v1/users/not-a-uuid",
        Some(&token),
        None,
    )
    .await;
    assert_error(status, &res, ErrorCode::BadRequest);
    let (status, res) = call(&pool, Method::DELETE, &user, Some(&token), None).await;
    assert_error(status, &res, ErrorCode::BadRequest);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn concurrent_admin_deactivation_keeps_one(pool: PgPool) {
    let (a, a_token) = admin(&pool).await;
    let (b, b_token) = user_with_token(&pool, "admin2", role::ADMIN).await;

    // 兩位管理者同時停用對方：只有一個會成功
    let (ra, rb) = tokio::join!(
        patch(
            &pool,
            &a_token,
            b,
            json!({ "version": 1, "is_active": false })
        ),
        patch(
            &pool,
            &b_token,
            a,
            json!({ "version": 1, "is_active": false })
        ),
    );
    let mut statuses = [ra.0, rb.0];
    statuses.sort();
    assert!(
        statuses == [StatusCode::OK, StatusCode::UNAUTHORIZED]
            || statuses == [StatusCode::OK, StatusCode::UNPROCESSABLE_ENTITY],
        "{ra:?} {rb:?}"
    );
    let active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM users u JOIN roles r ON r.id = u.role_id
         WHERE r.code = 'admin' AND u.is_active AND u.deleted_at IS NULL",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(active, 1);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn requires_password_change_first(pool: PgPool) {
    // 必須先修改密碼的管理者不能使用帳號管理
    let (_, token) = create_user_with_session(
        &pool,
        &TestUser::new("admin", role::ADMIN)
            .password(PASSWORD)
            .must_change_password(),
    )
    .await
    .unwrap();
    let (status, body) = call(
        &pool,
        Method::GET,
        "/api/v1/users",
        Some(token.as_str()),
        None,
    )
    .await;
    assert_error(status, &body, ErrorCode::PasswordChangeRequired);
    let (status, body) = call(&pool, Method::GET, "/api/v1/users", None, None).await;
    assert_error(status, &body, ErrorCode::Unauthorized);
}

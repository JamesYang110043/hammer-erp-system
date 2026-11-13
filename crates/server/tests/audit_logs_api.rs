//! 操作紀錄查詢 API 的整合測試（#28）。
//!
//! 使用 `#[sqlx::test]`，需要 `DATABASE_URL`（見 README「整合測試」）。
//! 透過完整的 router 呼叫，包含所有中介層。
//!
//! 測試資料的時間都在 2020 年，查詢時以 `from` / `to` 限定在這段期間，
//! 避免建立帳號、登入等動作另外寫入的紀錄影響結果。

#![allow(clippy::unwrap_used)]

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use hammer_core::test_util::{TestUser, create_user_with_session};
use hammer_server::{AppState, Config, router};
use hammer_shared::audit::AuditLogEntry;
use hammer_shared::pagination::Page;
use hammer_shared::{ApiError, ErrorCode, Id, audit, new_id, permission, role};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use time::macros::datetime;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;

/// 測試資料所在期間。
const WINDOW: &str = "from=2020-01-01T00:00:00Z&to=2021-01-01T00:00:00Z";

fn app(pool: &PgPool) -> Router {
    let config = Config::from_sources(None, |name| match name {
        "DATABASE_URL" => Some("postgres://unused".to_owned()),
        _ => None,
    })
    .unwrap();
    router(AppState::new(pool.clone(), config))
}

async fn get(pool: &PgPool, uri: &str, token: Option<&str>) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::USER_AGENT, "HammerERP-test")
        .header("X-Client-Version", "0.1.0");
    if let Some(token) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let res = app(pool)
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, value)
}

/// 以 `query`（不含 `?`）查詢，必須成功。
async fn list(pool: &PgPool, token: &str, query: &str) -> Page<AuditLogEntry> {
    let (status, body) = get(pool, &format!("/api/v1/audit-logs?{query}"), Some(token)).await;
    assert_eq!(status, StatusCode::OK, "{query}: {body}");
    serde_json::from_value(body).unwrap()
}

fn assert_error(status: StatusCode, body: &Value, expected: ErrorCode) -> ApiError {
    assert_eq!(status.as_u16(), expected.http_status(), "{body}");
    let err: ApiError = serde_json::from_value(body.clone()).unwrap();
    assert_eq!(err.code, expected, "{body}");
    assert!(!err.message.is_empty());
    err
}

async fn user_with_token(pool: &PgPool, username: &str, role: &str) -> (Id, String) {
    let (id, token) = create_user_with_session(pool, &TestUser::new(username, role))
        .await
        .unwrap();
    (id, token.into_string())
}

/// 寫入一筆操作紀錄，回傳 id。
async fn insert_log(
    pool: &PgPool,
    occurred_at: OffsetDateTime,
    user_id: Option<Id>,
    action: &str,
    entity_key: Option<&str>,
    record_id: Option<Id>,
) -> Id {
    let id = new_id();
    sqlx::query(
        "INSERT INTO audit_logs
             (id, occurred_at, user_id, action, entity_key, record_id, after_data, client_info)
         VALUES ($1, $2, $3, $4, $5, $6, $7, 'HammerERP-test')",
    )
    .bind(id)
    .bind(occurred_at)
    .bind(user_id)
    .bind(action)
    .bind(entity_key)
    .bind(record_id)
    .bind(json!({ "n": 1 }))
    .execute(pool)
    .await
    .unwrap();
    id
}

fn ids(page: &Page<AuditLogEntry>) -> Vec<Id> {
    page.items.iter().map(|e| e.id).collect()
}

/// 篩選測試用的固定資料。
struct Fixture {
    admin_token: String,
    admin_id: Id,
    editor_id: Id,
    record_x: Id,
    record_y: Id,
    /// 依時間由舊到新：
    /// 1. 01-01 admin  create       users    X
    /// 2. 01-02 editor update       users    X
    /// 3. 01-03 editor login        —        —
    /// 4. 01-04 （無）  login_failed users    —
    /// 5. 01-05 admin  update       products Y
    logs: [Id; 5],
}

async fn fixture(pool: &PgPool) -> Fixture {
    let (admin_id, admin_token) = user_with_token(pool, "admin", role::ADMIN).await;
    let (editor_id, _) = user_with_token(pool, "editor", role::EDITOR).await;
    let record_x = new_id();
    let record_y = new_id();
    let day = |d: u8| datetime!(2020-01-01 00:00 UTC) + Duration::days(i64::from(d) - 1);
    let logs = [
        insert_log(
            pool,
            day(1),
            Some(admin_id),
            audit::CREATE,
            Some("users"),
            Some(record_x),
        )
        .await,
        insert_log(
            pool,
            day(2),
            Some(editor_id),
            audit::UPDATE,
            Some("users"),
            Some(record_x),
        )
        .await,
        insert_log(pool, day(3), Some(editor_id), audit::LOGIN, None, None).await,
        insert_log(pool, day(4), None, audit::LOGIN_FAILED, Some("users"), None).await,
        insert_log(
            pool,
            day(5),
            Some(admin_id),
            audit::UPDATE,
            Some("products"),
            Some(record_y),
        )
        .await,
    ];
    Fixture {
        admin_token,
        admin_id,
        editor_id,
        record_x,
        record_y,
        logs,
    }
}

// ---------------------------------------------------------------------------
// 權限
// ---------------------------------------------------------------------------

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn editor_and_viewer_are_forbidden(pool: PgPool) {
    for (username, role) in [("editor", role::EDITOR), ("viewer", role::VIEWER)] {
        let (_, token) = user_with_token(&pool, username, role).await;
        let (status, body) = get(&pool, "/api/v1/audit-logs", Some(&token)).await;
        let err = assert_error(status, &body, ErrorCode::Forbidden);
        assert_eq!(
            err.details,
            Some(json!({ "permission": permission::AUDIT_READ })),
            "{role}"
        );
    }
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn requires_login(pool: PgPool) {
    let (status, body) = get(&pool, "/api/v1/audit-logs", None).await;
    assert_error(status, &body, ErrorCode::Unauthorized);
}

// ---------------------------------------------------------------------------
// 回應內容與篩選
// ---------------------------------------------------------------------------

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn returns_newest_first_with_user_names(pool: PgPool) {
    let f = fixture(&pool).await;
    let page = list(&pool, &f.admin_token, WINDOW).await;
    let [l1, l2, l3, l4, l5] = f.logs;
    assert_eq!(ids(&page), vec![l5, l4, l3, l2, l1]);
    assert_eq!((page.total, page.page, page.page_size), (5, 1, 50));

    let newest = &page.items[0];
    assert_eq!(newest.occurred_at, datetime!(2020-01-05 00:00 UTC));
    assert_eq!(newest.user_id, Some(f.admin_id));
    assert_eq!(newest.username.as_deref(), Some("admin"));
    assert_eq!(newest.user_display_name.as_deref(), Some("admin"));
    assert_eq!(newest.action, audit::UPDATE);
    assert_eq!(newest.entity_key.as_deref(), Some("products"));
    assert_eq!(newest.record_id, Some(f.record_y));
    assert_eq!(newest.before_data, None);
    assert_eq!(newest.after_data, Some(json!({ "n": 1 })));
    assert_eq!(newest.client_info.as_deref(), Some("HammerERP-test"));

    // 沒有操作者的紀錄（帳號不存在的登入失敗）
    let anonymous = &page.items[1];
    assert_eq!(anonymous.action, audit::LOGIN_FAILED);
    assert_eq!(
        (
            &anonymous.user_id,
            &anonymous.username,
            &anonymous.user_display_name
        ),
        (&None, &None, &None)
    );
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn time_range_filter(pool: PgPool) {
    let f = fixture(&pool).await;
    let [_, l2, l3, l4, _] = f.logs;
    // from 含、to 不含
    let page = list(
        &pool,
        &f.admin_token,
        "from=2020-01-02T00:00:00Z&to=2020-01-05T00:00:00Z",
    )
    .await;
    assert_eq!(ids(&page), vec![l4, l3, l2]);
    assert_eq!(page.total, 3);

    // 時區位移（`+` 需編碼為 %2B）：台北 01-04 08:00 = UTC 01-04 00:00
    let page = list(
        &pool,
        &f.admin_token,
        "from=2020-01-04T08:00:00%2B08:00&to=2021-01-01T00:00:00Z",
    )
    .await;
    assert_eq!(page.total, 2);

    // 只有 from 或只有 to
    let page = list(&pool, &f.admin_token, "from=2020-01-05T00:00:00Z").await;
    assert_eq!(ids(&page).last(), Some(&f.logs[4]));
    let page = list(&pool, &f.admin_token, "to=2020-01-02T00:00:00Z").await;
    assert_eq!(ids(&page), vec![f.logs[0]]);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn user_filter(pool: PgPool) {
    let f = fixture(&pool).await;
    let [l1, l2, l3, _, l5] = f.logs;
    let page = list(
        &pool,
        &f.admin_token,
        &format!("{WINDOW}&user_id={}", f.editor_id),
    )
    .await;
    assert_eq!(ids(&page), vec![l3, l2]);
    assert!(
        page.items
            .iter()
            .all(|e| e.user_display_name.as_deref() == Some("editor"))
    );

    let page = list(
        &pool,
        &f.admin_token,
        &format!("{WINDOW}&user_id={}", f.admin_id),
    )
    .await;
    assert_eq!(ids(&page), vec![l5, l1]);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn action_filter(pool: PgPool) {
    let f = fixture(&pool).await;
    let [_, l2, _, l4, l5] = f.logs;
    let page = list(&pool, &f.admin_token, &format!("{WINDOW}&action=update")).await;
    assert_eq!(ids(&page), vec![l5, l2]);
    let page = list(
        &pool,
        &f.admin_token,
        &format!("{WINDOW}&action=login_failed"),
    )
    .await;
    assert_eq!(ids(&page), vec![l4]);
    let page = list(&pool, &f.admin_token, &format!("{WINDOW}&action=void")).await;
    assert_eq!((page.items.len(), page.total), (0, 0));
    // 空字串視為未指定
    let page = list(&pool, &f.admin_token, &format!("{WINDOW}&action=")).await;
    assert_eq!(page.total, 5);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn entity_and_record_filters(pool: PgPool) {
    let f = fixture(&pool).await;
    let [l1, l2, _, l4, l5] = f.logs;
    let page = list(&pool, &f.admin_token, &format!("{WINDOW}&entity_key=users")).await;
    assert_eq!(ids(&page), vec![l4, l2, l1]);
    let page = list(
        &pool,
        &f.admin_token,
        &format!("{WINDOW}&entity_key=products"),
    )
    .await;
    assert_eq!(ids(&page), vec![l5]);

    let page = list(
        &pool,
        &f.admin_token,
        &format!("{WINDOW}&record_id={}", f.record_x),
    )
    .await;
    assert_eq!(ids(&page), vec![l2, l1]);
    let page = list(
        &pool,
        &f.admin_token,
        &format!("{WINDOW}&record_id={}", f.record_y),
    )
    .await;
    assert_eq!(ids(&page), vec![l5]);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn combined_filters(pool: PgPool) {
    let f = fixture(&pool).await;
    let page = list(
        &pool,
        &f.admin_token,
        &format!(
            "{WINDOW}&entity_key=users&record_id={}&action=update&user_id={}",
            f.record_x, f.editor_id
        ),
    )
    .await;
    assert_eq!(ids(&page), vec![f.logs[1]]);
    assert_eq!(page.total, 1);

    // 條件須全部符合
    let page = list(
        &pool,
        &f.admin_token,
        &format!("{WINDOW}&entity_key=products&user_id={}", f.editor_id),
    )
    .await;
    assert_eq!(page.total, 0);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn invalid_params(pool: PgPool) {
    let (_, token) = user_with_token(&pool, "admin", role::ADMIN).await;
    for (query, field) in [
        ("action=Login", "action"),
        ("from=2020-01-02T00:00:00Z&to=2020-01-01T00:00:00Z", "from"),
        ("page=0", "page"),
        ("page_size=0", "page_size"),
    ] {
        let (status, body) = get(&pool, &format!("/api/v1/audit-logs?{query}"), Some(&token)).await;
        let err = assert_error(status, &body, ErrorCode::Validation);
        assert_eq!(err.details, Some(json!({ "field": field })), "{query}");
    }

    for query in [
        "user_id=not-a-uuid",
        "from=2020-01-01",
        "page=-1",
        "page_size=abc",
    ] {
        let (status, body) = get(&pool, &format!("/api/v1/audit-logs?{query}"), Some(&token)).await;
        assert_error(status, &body, ErrorCode::BadRequest);
    }
}

// ---------------------------------------------------------------------------
// 分頁
// ---------------------------------------------------------------------------

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn pagination(pool: PgPool) {
    let (admin_id, token) = user_with_token(&pool, "admin", role::ADMIN).await;
    // 205 筆，依序相隔一分鐘；另外 3 筆同一時間，確認同時間的排序也穩定
    let start = datetime!(2020-03-01 00:00 UTC);
    let mut expected = Vec::new();
    for i in 0..205 {
        let at = start + Duration::minutes(i);
        expected.push(insert_log(&pool, at, Some(admin_id), audit::EXPORT, None, None).await);
    }
    let same_time = start + Duration::days(1);
    for _ in 0..3 {
        expected
            .push(insert_log(&pool, same_time, Some(admin_id), audit::EXPORT, None, None).await);
    }
    // 由新到舊；同時間依 id（uuid v7 依建立順序遞增）由新到舊
    expected.reverse();
    let total = 208;

    // 預設每頁 50 筆
    let page = list(&pool, &token, WINDOW).await;
    assert_eq!((page.total, page.page, page.page_size), (total, 1, 50));
    assert_eq!(ids(&page), expected[..50]);

    // 逐頁取得的結果不重複、不遺漏
    let mut all = Vec::new();
    for n in 1..=5 {
        let page = list(&pool, &token, &format!("{WINDOW}&page={n}&page_size=50")).await;
        assert_eq!((page.total, page.page, page.page_size), (total, n, 50));
        assert_eq!(page.items.len(), if n < 5 { 50 } else { 8 }, "page {n}");
        all.extend(ids(&page));
    }
    assert_eq!(all, expected);

    // page_size 超過上限時套用上限 200
    let page = list(&pool, &token, &format!("{WINDOW}&page_size=1000")).await;
    assert_eq!((page.total, page.page, page.page_size), (total, 1, 200));
    assert_eq!(ids(&page), expected[..200]);
    let page = list(&pool, &token, &format!("{WINDOW}&page=2&page_size=1000")).await;
    assert_eq!(page.page_size, 200);
    assert_eq!(ids(&page), expected[200..]);

    // 頁碼超過最後一頁：空陣列，總筆數不變
    let page = list(&pool, &token, &format!("{WINDOW}&page=99")).await;
    assert!(page.items.is_empty());
    assert_eq!(page.total, total);

    // 總筆數依篩選條件計算
    let page = list(
        &pool,
        &token,
        "from=2020-03-01T00:00:00Z&to=2020-03-01T00:10:00Z&page_size=3",
    )
    .await;
    assert_eq!((page.total, page.items.len()), (10, 3));
}

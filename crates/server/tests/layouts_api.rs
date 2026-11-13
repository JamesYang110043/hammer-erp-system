//! 列表版面（view_layouts）API 的整合測試（#51）。
//!
//! 使用 `#[sqlx::test]`，需要 `DATABASE_URL`（見 README「整合測試」）。
//! 透過完整的 router 呼叫，包含所有中介層。

#![allow(clippy::unwrap_used)]

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use hammer_core::test_util::{TestUser, create_user_with_session};
use hammer_server::{AppState, Config, router};
use hammer_shared::layout::{DefaultViewLayout, LayoutSource, ViewLayout};
use hammer_shared::schema::EntityDetail;
use hammer_shared::{ApiError, ErrorCode, Id, role};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

const ENTITY: &str = "repair_order";

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
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::USER_AGENT, "HammerERP-test")
        .header("X-Client-Version", "0.1.0")
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
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

/// 422 VALIDATION，且 `details.field` 為 `field`。
fn assert_invalid(status: StatusCode, body: &Value, field: &str) {
    let err = assert_error(status, body, ErrorCode::Validation);
    assert_eq!(err.details, Some(json!({ "field": field })), "{body}");
}

async fn token_for(pool: &PgPool, username: &str, role: &str) -> String {
    let (_, token) = create_user_with_session(pool, &TestUser::new(username, role))
        .await
        .unwrap();
    token.into_string()
}

/// 建立資料類別 [`ENTITY`] 與欄位，回傳管理者 token。
///
/// | 欄位 | 型態 | 設定 |
/// |---|---|---|
/// | `title` | text | 排序 1 |
/// | `qty` | integer | 排序 2，欄寬 80 |
/// | `note` | long_text | 排序 3，不顯示 |
/// | `status` | select | 排序 0 |
async fn setup(pool: &PgPool) -> String {
    let admin = token_for(pool, "admin", role::ADMIN).await;
    let (status, body) = call(
        pool,
        Method::POST,
        "/api/v1/entities",
        &admin,
        Some(json!({ "key": ENTITY, "label": "維修單", "kind": "master" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    for field in [
        json!({ "field_key": "title", "label": "標題", "data_type": "text", "sort_order": 1 }),
        json!({ "field_key": "qty", "label": "數量", "data_type": "integer", "sort_order": 2,
                "column_width": 80 }),
        json!({ "field_key": "note", "label": "備註", "data_type": "long_text", "sort_order": 3,
                "is_visible": false }),
        json!({ "field_key": "status", "label": "狀態", "data_type": "select", "sort_order": 0,
                "options": [{ "value": "open", "label": "開立" }] }),
    ] {
        let (status, body) = call(
            pool,
            Method::POST,
            &format!("/api/v1/entities/{ENTITY}/fields"),
            &admin,
            Some(field),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    admin
}

fn layouts_uri() -> String {
    format!("/api/v1/entities/{ENTITY}/layouts")
}

fn layout_uri(id: Id) -> String {
    format!("/api/v1/entities/{ENTITY}/layouts/{id}")
}

async fn create_raw(pool: &PgPool, token: &str, body: Value) -> (StatusCode, Value) {
    call(pool, Method::POST, &layouts_uri(), token, Some(body)).await
}

async fn create(pool: &PgPool, token: &str, body: Value) -> ViewLayout {
    let (status, body) = create_raw(pool, token, body).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn patch(pool: &PgPool, token: &str, id: Id, body: Value) -> (StatusCode, Value) {
    call(pool, Method::PATCH, &layout_uri(id), token, Some(body)).await
}

async fn delete(pool: &PgPool, token: &str, id: Id, version: i32) -> (StatusCode, Value) {
    call(
        pool,
        Method::DELETE,
        &format!("{}?version={version}", layout_uri(id)),
        token,
        None,
    )
    .await
}

async fn list(pool: &PgPool, token: &str) -> Vec<ViewLayout> {
    let (status, body) = call(pool, Method::GET, &layouts_uri(), token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn find(pool: &PgPool, token: &str, id: Id) -> ViewLayout {
    list(pool, token)
        .await
        .into_iter()
        .find(|l| l.id == id)
        .unwrap()
}

async fn default_layout(pool: &PgPool, token: &str) -> DefaultViewLayout {
    let (status, body) = call(
        pool,
        Method::GET,
        &format!("{}/default", layouts_uri()),
        token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

fn column_keys(layout_config: &hammer_shared::layout::LayoutConfig) -> Vec<&str> {
    layout_config
        .columns
        .iter()
        .map(|c| c.field_key.as_str())
        .collect()
}

/// 操作紀錄：`(action, record_id, before, after)`，依寫入順序。
async fn layout_audits(pool: &PgPool) -> Vec<(String, Id, Option<Value>, Option<Value>)> {
    sqlx::query_as(
        "SELECT action, record_id, before_data, after_data FROM audit_logs
         WHERE entity_key = 'view_layout' ORDER BY occurred_at, id",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

// ---------------------------------------------------------------------------
// 權限
// ---------------------------------------------------------------------------

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn viewer_can_create_own_layout_but_not_global(pool: PgPool) {
    let admin = setup(&pool).await;
    let viewer = token_for(&pool, "viewer", role::VIEWER).await;
    let editor = token_for(&pool, "editor", role::EDITOR).await;

    let mine = create(
        &pool,
        &viewer,
        json!({
            "name": "  我的版面 ",
            "config": {
                "columns": [{ "field_key": "qty", "width": 120 }, { "field_key": "title" }],
                "sort": { "field_key": "qty", "descending": true },
                "filters": [{ "field_key": "title", "op": "contains", "value": "王" }],
            },
        }),
    )
    .await;
    assert_eq!(mine.name, "我的版面");
    assert!(mine.user_id.is_some() && !mine.is_global());
    assert!(!mine.is_default);
    assert_eq!(mine.version, 1);
    assert_eq!(column_keys(&mine.config), ["qty", "title"]);
    assert_eq!(mine.config.columns[0].width, Some(120));
    assert_eq!(mine.config.filters.len(), 1);

    // 可編輯者、唯讀者都不能建立全域版面
    for token in [&viewer, &editor] {
        let (status, body) =
            create_raw(&pool, token, json!({ "name": "全域", "is_global": true })).await;
        assert_error(status, &body, ErrorCode::Forbidden);
    }

    let global = create(&pool, &admin, json!({ "name": "全域", "is_global": true })).await;
    assert!(global.is_global());

    // 唯讀者看得到全域版面（排在前面）與自己的版面，但不能修改或刪除全域版面
    let ids: Vec<Id> = list(&pool, &viewer).await.iter().map(|l| l.id).collect();
    assert_eq!(ids, [global.id, mine.id]);
    let (status, body) = patch(
        &pool,
        &viewer,
        global.id,
        json!({ "version": 1, "name": "x" }),
    )
    .await;
    assert_error(status, &body, ErrorCode::Forbidden);
    let (status, body) = delete(&pool, &viewer, global.id, 1).await;
    assert_error(status, &body, ErrorCode::Forbidden);

    // 唯讀者可以修改、刪除自己的版面
    let (status, body) = patch(
        &pool,
        &viewer,
        mine.id,
        json!({ "version": 1, "name": "改名" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["name"], "改名");
    assert_eq!(body["version"], 2);
    let (status, body) = delete(&pool, &viewer, mine.id, 2).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let ids: Vec<Id> = list(&pool, &viewer).await.iter().map(|l| l.id).collect();
    assert_eq!(ids, [global.id]);

    // 管理者可以修改全域版面
    let (status, body) = patch(
        &pool,
        &admin,
        global.id,
        json!({ "version": 1, "name": "全域 2" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn users_cannot_modify_others_layouts(pool: PgPool) {
    let admin = setup(&pool).await;
    let alice = token_for(&pool, "alice", role::EDITOR).await;
    let bob = token_for(&pool, "bob", role::EDITOR).await;

    let layout = create(&pool, &alice, json!({ "name": "alice 的版面" })).await;

    // 別人（含管理者）看不到，也不能修改或刪除
    for token in [&bob, &admin] {
        assert!(list(&pool, token).await.is_empty());
        let (status, body) = patch(
            &pool,
            token,
            layout.id,
            json!({ "version": 1, "name": "x" }),
        )
        .await;
        assert_error(status, &body, ErrorCode::NotFound);
        let (status, body) = patch(
            &pool,
            token,
            layout.id,
            json!({ "version": 1, "is_default": true }),
        )
        .await;
        assert_error(status, &body, ErrorCode::NotFound);
        let (status, body) = delete(&pool, token, layout.id, 1).await;
        assert_error(status, &body, ErrorCode::NotFound);
    }

    let unchanged = find(&pool, &alice, layout.id).await;
    assert_eq!(unchanged, layout);
}

// ---------------------------------------------------------------------------
// 預設版面
// ---------------------------------------------------------------------------

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn default_layout_resolution_order(pool: PgPool) {
    let admin = setup(&pool).await;
    let viewer = token_for(&pool, "viewer", role::VIEWER).await;

    // 沒有任何預設 → 依欄位定義產生：顯示的欄位依排序，欄寬為欄位定義的欄寬
    let generated = default_layout(&pool, &viewer).await;
    assert_eq!(generated.source, LayoutSource::Generated);
    assert!(generated.layout.is_none());
    assert_eq!(column_keys(&generated.config), ["status", "title", "qty"]);
    assert_eq!(generated.config.columns[2].width, Some(80));
    assert!(generated.config.sort.is_none() && generated.config.filters.is_empty());

    // 不是預設的版面不影響
    create(&pool, &viewer, json!({ "name": "一般" })).await;
    create(
        &pool,
        &admin,
        json!({ "name": "全域一般", "is_global": true }),
    )
    .await;
    assert_eq!(
        default_layout(&pool, &viewer).await.source,
        LayoutSource::Generated
    );

    // 全域預設
    let global = create(
        &pool,
        &admin,
        json!({ "name": "全域預設", "is_global": true, "is_default": true,
                "config": { "columns": [{ "field_key": "title" }] } }),
    )
    .await;
    let resolved = default_layout(&pool, &viewer).await;
    assert_eq!(resolved.source, LayoutSource::Global);
    assert_eq!(resolved.layout.as_ref().unwrap().id, global.id);
    assert_eq!(column_keys(&resolved.config), ["title"]);

    // 使用者自己的預設優先
    let personal = create(
        &pool,
        &viewer,
        json!({ "name": "我的預設", "is_default": true,
                "config": { "columns": [{ "field_key": "qty" }] } }),
    )
    .await;
    let resolved = default_layout(&pool, &viewer).await;
    assert_eq!(resolved.source, LayoutSource::Personal);
    assert_eq!(resolved.layout.as_ref().unwrap().id, personal.id);
    assert_eq!(column_keys(&resolved.config), ["qty"]);
    // 別的使用者仍然取得全域預設
    assert_eq!(
        default_layout(&pool, &admin).await.source,
        LayoutSource::Global
    );

    // 刪除自己的預設 → 回到全域預設
    let (status, body) = delete(&pool, &viewer, personal.id, personal.version).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(
        default_layout(&pool, &viewer).await.source,
        LayoutSource::Global
    );

    // 取消全域預設 → 依欄位定義產生
    let (status, body) = patch(
        &pool,
        &admin,
        global.id,
        json!({ "version": global.version, "is_default": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        default_layout(&pool, &viewer).await.source,
        LayoutSource::Generated
    );
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn setting_new_default_clears_previous_default(pool: PgPool) {
    let admin = setup(&pool).await;
    let viewer = token_for(&pool, "viewer", role::VIEWER).await;

    let first = create(&pool, &viewer, json!({ "name": "A", "is_default": true })).await;
    assert!(first.is_default);
    // 其他範圍的預設版面：全域預設、別的使用者的預設
    let global = create(
        &pool,
        &admin,
        json!({ "name": "全域", "is_global": true, "is_default": true }),
    )
    .await;
    let admins = create(
        &pool,
        &admin,
        json!({ "name": "管理者的", "is_default": true }),
    )
    .await;

    // 新增時設為預設 → 原本的預設被取消（version 遞增）
    let second = create(&pool, &viewer, json!({ "name": "B", "is_default": true })).await;
    assert!(second.is_default);
    let first_now = find(&pool, &viewer, first.id).await;
    assert!(!first_now.is_default);
    assert_eq!(first_now.version, first.version + 1);

    // 修改時設為預設 → 同樣取消原本的預設
    let (status, body) = patch(
        &pool,
        &viewer,
        first.id,
        json!({ "version": first_now.version, "is_default": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["is_default"], true);
    let defaults: Vec<Id> = list(&pool, &viewer)
        .await
        .iter()
        .filter(|l| l.is_default && !l.is_global())
        .map(|l| l.id)
        .collect();
    assert_eq!(defaults, [first.id]);
    assert_eq!(
        default_layout(&pool, &viewer).await.layout.unwrap().id,
        first.id
    );

    // 其他範圍不受影響
    assert!(find(&pool, &admin, global.id).await.is_default);
    assert!(find(&pool, &admin, admins.id).await.is_default);

    // 全域預設也只有一個
    let global2 = create(
        &pool,
        &admin,
        json!({ "name": "全域 2", "is_global": true, "is_default": true }),
    )
    .await;
    assert!(!find(&pool, &admin, global.id).await.is_default);
    assert!(find(&pool, &admin, global2.id).await.is_default);
    assert!(find(&pool, &admin, admins.id).await.is_default);

    // 取消原本預設時也留下操作紀錄
    let audits = layout_audits(&pool).await;
    let cleared: Vec<Id> = audits
        .iter()
        .filter(|(action, _, before, after)| {
            action == "update"
                && before.as_ref() == Some(&json!({ "is_default": true }))
                && after.as_ref() == Some(&json!({ "is_default": false }))
        })
        .map(|(_, id, _, _)| *id)
        .collect();
    assert_eq!(cleared, [first.id, second.id, global.id]);
}

// ---------------------------------------------------------------------------
// config、版本、操作紀錄
// ---------------------------------------------------------------------------

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn invalid_input_is_rejected(pool: PgPool) {
    let _admin = setup(&pool).await;
    let viewer = token_for(&pool, "viewer", role::VIEWER).await;

    let cases = [
        (json!({ "name": " " }), "name"),
        (
            json!({ "name": "x", "config": { "columns": [{ "field_key": "missing" }] } }),
            "config.columns[0].field_key",
        ),
        (
            json!({ "name": "x", "config": { "columns": [{ "field_key": "qty", "width": -1 }] } }),
            "config.columns[0].width",
        ),
        (
            json!({ "name": "x", "config": { "sort": { "field_key": "missing", "descending": false } } }),
            "config.sort",
        ),
        (
            json!({ "name": "x", "config": { "filters": [
                { "field_key": "qty", "op": "gte", "value": "1" },
                { "field_key": "title", "op": "gt", "value": "a" },
            ] } }),
            "config.filters[1]",
        ),
    ];
    for (body, field) in cases {
        let (status, res) = create_raw(&pool, &viewer, body).await;
        assert_invalid(status, &res, field);
    }

    // 資料類別不存在
    let (status, body) = call(
        &pool,
        Method::GET,
        "/api/v1/entities/missing/layouts",
        &viewer,
        None,
    )
    .await;
    assert_error(status, &body, ErrorCode::NotFound);
    assert!(layout_audits(&pool).await.is_empty());
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn deactivated_fields_are_skipped_when_reading(pool: PgPool) {
    let admin = setup(&pool).await;
    let viewer = token_for(&pool, "viewer", role::VIEWER).await;

    let layout = create(
        &pool,
        &viewer,
        json!({ "name": "x", "is_default": true, "config": {
            "columns": [{ "field_key": "qty" }, { "field_key": "title" }],
            "sort": { "field_key": "qty", "descending": false },
            "filters": [
                { "field_key": "qty", "op": "gte", "value": "1" },
                { "field_key": "title", "op": "contains", "value": "a" },
            ],
        } }),
    )
    .await;

    // 停用 qty
    let (status, body) = call(
        &pool,
        Method::GET,
        &format!("/api/v1/entities/{ENTITY}"),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let detail: EntityDetail = serde_json::from_value(body).unwrap();
    let qty = detail.fields.iter().find(|f| f.field_key == "qty").unwrap();
    let (status, body) = call(
        &pool,
        Method::POST,
        &format!("/api/v1/entities/{ENTITY}/fields/qty/deactivate"),
        &admin,
        Some(json!({ "version": qty.version })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let read = find(&pool, &viewer, layout.id).await;
    assert_eq!(column_keys(&read.config), ["title"]);
    assert!(read.config.sort.is_none());
    assert_eq!(read.config.filters.len(), 1);
    assert_eq!(read.config.filters[0].field_key, "title");
    let resolved = default_layout(&pool, &viewer).await;
    assert_eq!(resolved.config, read.config);

    // 只改名稱時，不會因為儲存的設定參照停用的欄位而失敗
    let (status, body) = patch(
        &pool,
        &viewer,
        layout.id,
        json!({ "version": 1, "name": "y" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn version_is_checked_and_writes_are_audited(pool: PgPool) {
    let _admin = setup(&pool).await;
    let viewer = token_for(&pool, "viewer", role::VIEWER).await;

    let layout = create(&pool, &viewer, json!({ "name": "A" })).await;

    // 沒有變動 → 不寫入
    let (status, body) = patch(
        &pool,
        &viewer,
        layout.id,
        json!({ "version": 1, "name": "A" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["version"], 1);

    let (status, body) = patch(
        &pool,
        &viewer,
        layout.id,
        json!({ "version": 1, "config": { "columns": [{ "field_key": "title" }] } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["version"], 2);

    // 舊版本號 → 409
    let (status, body) = patch(
        &pool,
        &viewer,
        layout.id,
        json!({ "version": 1, "name": "B" }),
    )
    .await;
    assert_error(status, &body, ErrorCode::Conflict);
    let (status, body) = delete(&pool, &viewer, layout.id, 1).await;
    assert_error(status, &body, ErrorCode::Conflict);

    let (status, body) = delete(&pool, &viewer, layout.id, 2).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    // 已刪除 → 404
    let (status, body) = delete(&pool, &viewer, layout.id, 3).await;
    assert_error(status, &body, ErrorCode::NotFound);

    let audits = layout_audits(&pool).await;
    let actions: Vec<&str> = audits.iter().map(|(a, ..)| a.as_str()).collect();
    assert_eq!(actions, ["create", "update", "delete"]);
    assert!(audits.iter().all(|(_, id, ..)| *id == layout.id));
    let (_, _, before, after) = &audits[1];
    assert_eq!(
        before.as_ref().unwrap()["config"],
        json!({ "columns": [], "sort": null, "filters": [] })
    );
    assert_eq!(
        after.as_ref().unwrap(),
        &json!({ "config": { "columns": [{ "field_key": "title", "width": null }], "sort": null, "filters": [] } })
    );
    assert_eq!(audits[0].3.as_ref().unwrap()["entity_key"], ENTITY);
}

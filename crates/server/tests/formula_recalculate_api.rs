//! 重新計算公式的 API 整合測試（#92）。
//!
//! - 修改公式後重新計算：所有資料（含明細）的值更新，回傳筆數正確；已刪除的資料不處理
//! - 關聯目標修改後重新計算：引用它的公式更新
//! - 值沒變動的資料 `version` 不變；有變動的 + 1；再執行一次沒有任何變動
//! - 計算錯誤的筆數
//! - 超過一批（500 筆）時分批處理
//! - 沒有 `schema.manage` → 403；明細子表、不存在的資料類別 → 404
//! - 寫入一筆 `audit_logs`，不逐筆寫
//!
//! 逐筆的計算與比較在 `hammer_core::recalculate` 的單元測試。
//! 使用 `#[sqlx::test]`，需要 `DATABASE_URL`（見 README「整合測試」）。

#![allow(clippy::unwrap_used)]

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use hammer_core::test_util::{TestUser, create_user_with_session};
use hammer_server::{AppState, Config, router};
use hammer_shared::record::Record;
use hammer_shared::schema::{EntityDetail, FieldDefinition, RecalculateResult};
use hammer_shared::{Id, role};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

/// 關聯目標（客戶）。
const CUSTOMER: &str = "customer";
/// 有公式欄位的主表（訂單）。
const ORDER: &str = "sales_order";
/// 訂單的明細子表（品項）。
const ITEMS: &str = "items";

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

async fn create_entity(pool: &PgPool, admin: &str, body: Value) -> EntityDetail {
    let (status, body) = call(pool, Method::POST, "/api/v1/entities", admin, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn create_field(pool: &PgPool, admin: &str, entity: &str, field: Value) {
    let (status, body) = call(
        pool,
        Method::POST,
        &format!("/api/v1/entities/{entity}/fields"),
        admin,
        Some(field),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

/// 公式欄位的 request。`money` 結果時小數位數 2。
fn formula_field(key: &str, formula: &str, result: &str) -> Value {
    let mut body = json!({
        "field_key": key,
        "label": format!("{key} 公式"),
        "data_type": "formula",
        "formula": formula,
        "formula_result_type": result,
    });
    if result == "money" {
        body["decimal_places"] = json!(2);
    }
    body
}

/// 修改公式欄位的公式（其他屬性維持原值）。
async fn change_formula(pool: &PgPool, admin: &str, entity: &str, key: &str, formula: &str) {
    let (status, body) = call(
        pool,
        Method::GET,
        &format!("/api/v1/entities/{entity}"),
        admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let detail: EntityDetail = serde_json::from_value(body).unwrap();
    let field: &FieldDefinition = detail.fields.iter().find(|f| f.field_key == key).unwrap();
    let mut req = serde_json::to_value(field).unwrap();
    req["formula"] = json!(formula);
    let (status, body) = call(
        pool,
        Method::PATCH,
        &format!("/api/v1/entities/{entity}/fields/{key}"),
        admin,
        Some(req),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// 建立以下資料類別，回傳管理者 token。
///
/// | 資料類別 | 欄位 | 公式 |
/// |---|---|---|
/// | [`CUSTOMER`] | `name`（text）、`discount`（decimal） | |
/// | [`ITEMS`] | `qty`（integer）、`unit`（money） | `amount = {qty} * {unit}` |
/// | [`ORDER`] | `qty`（integer）、`price`（money）、`divisor`（integer）、`customer`（relation） | 見下方 |
///
/// - `subtotal = {qty} * {price}`
/// - `items_total = SUM(items.amount)`
/// - `discounted = {subtotal} * (1 - COALESCE({customer.discount}, 0))`
/// - `ratio = {qty} / {divisor}`（integer；`divisor` 為 0 時錯誤）
async fn setup(pool: &PgPool) -> String {
    let (_, token) = create_user_with_session(pool, &TestUser::new("admin", role::ADMIN))
        .await
        .unwrap();
    let admin = token.into_string();

    let customer = create_entity(
        pool,
        &admin,
        json!({ "key": CUSTOMER, "label": "客戶", "kind": "master" }),
    )
    .await;
    for f in [
        json!({ "field_key": "name", "label": "名稱", "data_type": "text" }),
        json!({ "field_key": "discount", "label": "折扣率", "data_type": "decimal", "decimal_places": 2 }),
    ] {
        create_field(pool, &admin, CUSTOMER, f).await;
    }

    let order = create_entity(
        pool,
        &admin,
        json!({ "key": ORDER, "label": "訂單", "kind": "document" }),
    )
    .await;
    for f in [
        json!({ "field_key": "qty", "label": "數量", "data_type": "integer" }),
        json!({ "field_key": "price", "label": "單價", "data_type": "money", "decimal_places": 2 }),
        json!({ "field_key": "divisor", "label": "除數", "data_type": "integer" }),
        json!({ "field_key": "customer", "label": "客戶", "data_type": "relation",
                "relation_entity_id": customer.entity.id }),
    ] {
        create_field(pool, &admin, ORDER, f).await;
    }

    create_entity(
        pool,
        &admin,
        json!({ "key": ITEMS, "label": "品項", "kind": "detail",
                "parent_entity_id": order.entity.id }),
    )
    .await;
    for f in [
        json!({ "field_key": "qty", "label": "數量", "data_type": "integer" }),
        json!({ "field_key": "unit", "label": "單價", "data_type": "money", "decimal_places": 2 }),
        formula_field("amount", "{qty} * {unit}", "money"),
    ] {
        create_field(pool, &admin, ITEMS, f).await;
    }

    for (key, formula, result) in [
        ("subtotal", "{qty} * {price}", "money"),
        ("items_total", "SUM(items.amount)", "money"),
        (
            "discounted",
            "{subtotal} * (1 - COALESCE({customer.discount}, 0))",
            "money",
        ),
        ("ratio", "{qty} / {divisor}", "integer"),
    ] {
        create_field(pool, &admin, ORDER, formula_field(key, formula, result)).await;
    }
    admin
}

fn records_uri(entity: &str) -> String {
    format!("/api/v1/entities/{entity}/records")
}

fn recalculate_uri(entity: &str) -> String {
    format!("/api/v1/entities/{entity}/recalculate")
}

async fn create(pool: &PgPool, token: &str, entity: &str, body: Value) -> Record {
    let (status, body) = call(pool, Method::POST, &records_uri(entity), token, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn get(pool: &PgPool, token: &str, entity: &str, id: Id) -> Record {
    let (status, body) = call(
        pool,
        Method::GET,
        &format!("{}/{id}", records_uri(entity)),
        token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn recalculate(pool: &PgPool, token: &str, entity: &str) -> RecalculateResult {
    let (status, body) = call(pool, Method::POST, &recalculate_uri(entity), token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

/// 欄位的值；沒有值時為 `null`。
fn value(record: &Record, key: &str) -> Value {
    record.data.get(key).cloned().unwrap_or(Value::Null)
}

/// 明細某個欄位的值，依順序。
fn column(record: &Record, field: &str) -> Vec<Value> {
    record.lines.as_ref().unwrap()[ITEMS]
        .iter()
        .map(|l| value(l, field))
        .collect()
}

/// 明細的 `version`，依順序。
fn line_versions(record: &Record) -> Vec<i32> {
    record.lines.as_ref().unwrap()[ITEMS]
        .iter()
        .map(|l| l.version)
        .collect()
}

/// `action` 的操作紀錄筆數。
async fn audit_count(pool: &PgPool, action: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM audit_logs WHERE action = $1")
        .bind(action)
        .fetch_one(pool)
        .await
        .unwrap()
}

// ---------------------------------------------------------------------------
// 驗收條件
// ---------------------------------------------------------------------------

/// 修改主表與明細的公式後重新計算：所有資料（含明細）的值更新，回傳筆數正確，`version` + 1；
/// 已刪除的資料不處理。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn recalculates_master_and_lines_after_formula_change(pool: PgPool) {
    let admin = setup(&pool).await;
    let a = create(
        &pool,
        &admin,
        ORDER,
        json!({
            "data": { "qty": 2, "price": "10", "divisor": 1 },
            "lines": { ITEMS: [
                { "data": { "qty": 1, "unit": "5" } },
                { "data": { "qty": 3, "unit": "2" } },
            ] },
        }),
    )
    .await;
    let b = create(
        &pool,
        &admin,
        ORDER,
        json!({ "data": { "qty": 4, "price": "1.5", "divisor": 2 } }),
    )
    .await;
    let deleted = create(
        &pool,
        &admin,
        ORDER,
        json!({ "data": { "qty": 1, "price": "1", "divisor": 1 } }),
    )
    .await;
    let (status, _) = call(
        &pool,
        Method::DELETE,
        &format!(
            "{}/{}?version={}",
            records_uri(ORDER),
            deleted.id,
            deleted.version
        ),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(value(&a, "subtotal"), json!("20.00"));
    assert_eq!(column(&a, "amount"), vec![json!("5.00"), json!("6.00")]);
    assert_eq!(value(&a, "items_total"), json!("11.00"));

    // 修改公式後，既有資料的值不變
    change_formula(&pool, &admin, ORDER, "subtotal", "{qty} * {price} + 1").await;
    change_formula(&pool, &admin, ITEMS, "amount", "{qty} * {unit} * 10").await;
    let before = get(&pool, &admin, ORDER, a.id).await;
    assert_eq!(value(&before, "subtotal"), json!("20.00"));
    assert_eq!(
        column(&before, "amount"),
        vec![json!("5.00"), json!("6.00")]
    );

    let result = recalculate(&pool, &admin, ORDER).await;
    assert_eq!(
        result,
        RecalculateResult {
            processed: 2,
            changed: 2,
            with_errors: 0,
        }
    );

    let a2 = get(&pool, &admin, ORDER, a.id).await;
    assert_eq!(value(&a2, "subtotal"), json!("21.00"));
    assert_eq!(value(&a2, "discounted"), json!("21.00"));
    // 明細的公式一起更新，主表的彙總使用新的明細值
    assert_eq!(column(&a2, "amount"), vec![json!("50.00"), json!("60.00")]);
    assert_eq!(value(&a2, "items_total"), json!("110.00"));
    assert_eq!(a2.version, a.version + 1);
    assert_eq!(
        line_versions(&a2),
        line_versions(&a).iter().map(|v| v + 1).collect::<Vec<_>>()
    );

    let b2 = get(&pool, &admin, ORDER, b.id).await;
    assert_eq!(value(&b2, "subtotal"), json!("7.00"));
    assert_eq!(b2.version, b.version + 1);

    // 已刪除的資料不處理
    let (data, version): (Value, i32) =
        sqlx::query_as("SELECT data, version FROM custom_records WHERE id = $1")
            .bind(deleted.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(data["subtotal"], json!("1.00"));
    assert_eq!(version, deleted.version + 1); // 只有軟刪除時的 + 1
}

/// 值沒變動的資料 `version` 不變（明細也是）；有變動的 + 1。再執行一次沒有任何變動。
/// 關聯目標修改後，重新計算會更新引用它的公式。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn only_changed_records_are_written(pool: PgPool) {
    let admin = setup(&pool).await;
    let vip = create(
        &pool,
        &admin,
        CUSTOMER,
        json!({ "data": { "name": "VIP", "discount": "0.10" } }),
    )
    .await;
    let linked = create(
        &pool,
        &admin,
        ORDER,
        json!({
            "data": { "qty": 1, "price": "100", "divisor": 1, "customer": vip.id },
            "lines": { ITEMS: [ { "data": { "qty": 1, "unit": "1" } } ] },
        }),
    )
    .await;
    let plain = create(
        &pool,
        &admin,
        ORDER,
        json!({
            "data": { "qty": 1, "price": "100", "divisor": 1 },
            "lines": { ITEMS: [ { "data": { "qty": 1, "unit": "1" } } ] },
        }),
    )
    .await;
    assert_eq!(value(&linked, "discounted"), json!("90.00"));

    // 公式與資料都沒變：沒有任何寫入
    let result = recalculate(&pool, &admin, ORDER).await;
    assert_eq!(
        result,
        RecalculateResult {
            processed: 2,
            changed: 0,
            with_errors: 0,
        }
    );
    assert_eq!(
        get(&pool, &admin, ORDER, linked.id).await.version,
        linked.version
    );

    // 關聯目標修改：只有引用它的資料變動
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &format!("{}/{}", records_uri(CUSTOMER), vip.id),
        &admin,
        Some(json!({ "version": vip.version, "data": { "discount": "0.20" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        value(&get(&pool, &admin, ORDER, linked.id).await, "discounted"),
        json!("90.00")
    );

    let result = recalculate(&pool, &admin, ORDER).await;
    assert_eq!(
        result,
        RecalculateResult {
            processed: 2,
            changed: 1,
            with_errors: 0,
        }
    );
    let linked2 = get(&pool, &admin, ORDER, linked.id).await;
    assert_eq!(value(&linked2, "discounted"), json!("80.00"));
    assert_eq!(linked2.version, linked.version + 1);
    // 明細的值沒變動，不寫入
    assert_eq!(line_versions(&linked2), line_versions(&linked));

    let plain2 = get(&pool, &admin, ORDER, plain.id).await;
    assert_eq!(plain2.version, plain.version);
    assert_eq!(plain2.updated_at, plain.updated_at);
    assert_eq!(line_versions(&plain2), line_versions(&plain));

    // 再執行一次：沒有變動
    let result = recalculate(&pool, &admin, ORDER).await;
    assert_eq!(result.changed, 0);
    assert_eq!(
        get(&pool, &admin, ORDER, linked.id).await.version,
        linked2.version
    );
}

/// 計算失敗的資料計入 `with_errors`，錯誤原因寫入 `formula_errors`；修正公式後重新計算清除錯誤。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn counts_formula_errors(pool: PgPool) {
    let admin = setup(&pool).await;
    let ok = create(
        &pool,
        &admin,
        ORDER,
        json!({ "data": { "qty": 4, "price": "1", "divisor": 2 } }),
    )
    .await;
    let zero = create(
        &pool,
        &admin,
        ORDER,
        json!({ "data": { "qty": 4, "price": "1", "divisor": 0 } }),
    )
    .await;
    assert!(zero.formula_errors.contains_key("ratio"));

    let result = recalculate(&pool, &admin, ORDER).await;
    assert_eq!(
        result,
        RecalculateResult {
            processed: 2,
            changed: 0,
            with_errors: 1,
        }
    );

    change_formula(&pool, &admin, ORDER, "ratio", "{qty} / ({divisor} + 1)").await;
    let result = recalculate(&pool, &admin, ORDER).await;
    assert_eq!(
        result,
        RecalculateResult {
            processed: 2,
            changed: 2,
            with_errors: 0,
        }
    );
    let zero2 = get(&pool, &admin, ORDER, zero.id).await;
    assert!(zero2.formula_errors.is_empty());
    assert_eq!(value(&zero2, "ratio"), json!(4));
    let ok2 = get(&pool, &admin, ORDER, ok.id).await;
    assert_eq!(value(&ok2, "ratio"), json!(1));
}

/// 超過一批（500 筆）時分批處理，所有資料都更新。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn processes_more_than_one_batch(pool: PgPool) {
    let admin = setup(&pool).await;
    let entity_id: Id = sqlx::query_scalar("SELECT id FROM entity_definitions WHERE key = $1")
        .bind(ORDER)
        .fetch_one(&pool)
        .await
        .unwrap();
    // 直接寫入 501 筆尚未計算公式的資料
    sqlx::query(
        "INSERT INTO custom_records (id, entity_id, data)
         SELECT gen_random_uuid(), $1, jsonb_build_object('qty', n, 'price', '1', 'divisor', 1)
         FROM generate_series(1, 501) AS n",
    )
    .bind(entity_id)
    .execute(&pool)
    .await
    .unwrap();

    let result = recalculate(&pool, &admin, ORDER).await;
    assert_eq!(
        result,
        RecalculateResult {
            processed: 501,
            changed: 501,
            with_errors: 0,
        }
    );
    let missing: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM custom_records
         WHERE entity_id = $1 AND (data->>'subtotal')::numeric <> (data->>'qty')::numeric",
    )
    .bind(entity_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(missing, 0);
    assert_eq!(audit_count(&pool, "recalculate").await, 1);
}

/// 沒有 `schema.manage` → 403；明細子表、不存在的資料類別 → 404。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn permission_and_entity_checks(pool: PgPool) {
    let admin = setup(&pool).await;
    for role in [role::EDITOR, role::VIEWER] {
        let (_, token) = create_user_with_session(&pool, &TestUser::new(role, role))
            .await
            .unwrap();
        let (status, body) = call(
            &pool,
            Method::POST,
            &recalculate_uri(ORDER),
            token.as_str(),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{role}: {body}");
    }

    for key in [ITEMS, "no_such_entity"] {
        let (status, body) = call(&pool, Method::POST, &recalculate_uri(key), &admin, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{key}: {body}");
    }
    assert_eq!(audit_count(&pool, "recalculate").await, 0);
}

/// 操作紀錄的 `(entity_key, record_id, before_data, after_data)`。
type AuditRow = (Option<String>, Option<Id>, Option<Value>, Option<Value>);

/// 整批寫一筆 `audit_logs`（資料類別、筆數），不逐筆寫。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn writes_one_audit_log(pool: PgPool) {
    let admin = setup(&pool).await;
    for qty in 1..=3 {
        create(
            &pool,
            &admin,
            ORDER,
            json!({ "data": { "qty": qty, "price": "1", "divisor": 1 } }),
        )
        .await;
    }
    change_formula(&pool, &admin, ORDER, "subtotal", "{qty} * {price} * 2").await;
    let updates_before = audit_count(&pool, "update").await;

    recalculate(&pool, &admin, ORDER).await;

    let rows: Vec<AuditRow> = sqlx::query_as(
        "SELECT entity_key, record_id, before_data, after_data FROM audit_logs
         WHERE action = 'recalculate'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        vec![(
            Some(ORDER.to_owned()),
            None,
            None,
            Some(json!({ "processed": 3, "changed": 3, "with_errors": 0 })),
        )]
    );
    // 沒有逐筆的修改紀錄
    assert_eq!(audit_count(&pool, "update").await, updates_before);
}

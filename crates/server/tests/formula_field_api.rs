//! 公式欄位定義的 API 整合測試（#90）。
//!
//! - 建立、修改公式欄位：檢查語法、引用、型態與循環引用，寫入 `formula_deps`
//! - 公式欄位的屬性限制（不可重複、預設值、必填）
//! - 其他型態 ↔ `formula` 的型態變更被拒絕
//! - 被有效公式引用的欄位不能停用；被引用的明細子表不能停用、刪除
//! - 寫入 `schema_change_logs` 與 `audit_logs`
//!
//! 純邏輯（設定檢查、型態與循環檢查、標題格式化）在 `hammer_core::schema::rules`、
//! `hammer_core::schema::formula`、`hammer_core::title` 的單元測試。
//! 使用 `#[sqlx::test]`，需要 `DATABASE_URL`（見 README「整合測試」）。

#![allow(clippy::unwrap_used)]

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use hammer_core::test_util::{TestUser, create_user_with_session};
use hammer_server::{AppState, Config, router};
use hammer_shared::schema::{EntityDefinition, EntityDetail, FieldDefinition};
use hammer_shared::{ApiError, ErrorCode, Id, role};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

/// 關聯目標（客戶）。
const CUSTOMER: &str = "customer";
/// 有公式欄位的主表（維修單）。
const REPAIR: &str = "repair_order";
/// 維修單的明細子表（零件）。
const PARTS: &str = "parts";

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

/// 422 VALIDATION，回傳 `details`。
fn validation_details(status: StatusCode, body: &Value) -> Value {
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let err: ApiError = serde_json::from_value(body.clone()).unwrap();
    assert_eq!(err.code, ErrorCode::Validation, "{body}");
    assert!(!err.message.is_empty());
    err.details.unwrap()
}

/// 422 VALIDATION，且 `details.field` 為 `field`。
fn assert_invalid(status: StatusCode, body: &Value, field: &str) {
    let details = validation_details(status, body);
    assert_eq!(details["field"], json!(field), "{body}");
}

/// 公式錯誤：`details.field = formula`，帶 `span`（`[start, end]`），訊息包含 `message`。
fn assert_formula_span(status: StatusCode, body: &Value, span: (u64, u64), message: &str) {
    let details = validation_details(status, body);
    assert_eq!(details["field"], json!("formula"), "{body}");
    assert_eq!(
        details["span"],
        json!({ "start": span.0, "end": span.1 }),
        "{body}"
    );
    assert!(
        body["message"].as_str().unwrap().contains(message),
        "{body}"
    );
}

/// 被公式引用：`details.formulas` 依序為 `(entity_key, field_key)`。
fn assert_referenced_by(status: StatusCode, body: &Value, expected: &[(&str, &str)]) {
    let details = validation_details(status, body);
    assert_eq!(details["field"], json!("formula"), "{body}");
    let actual: Vec<(String, String)> = details["formulas"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            assert!(!f["field_label"].as_str().unwrap().is_empty(), "{body}");
            assert!(!f["entity_label"].as_str().unwrap().is_empty(), "{body}");
            (
                f["entity_key"].as_str().unwrap().to_owned(),
                f["field_key"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let expected: Vec<(String, String)> = expected
        .iter()
        .map(|(e, f)| ((*e).to_owned(), (*f).to_owned()))
        .collect();
    assert_eq!(actual, expected, "{body}");
}

async fn token_for(pool: &PgPool, username: &str, role: &str) -> String {
    let (_, token) = create_user_with_session(pool, &TestUser::new(username, role))
        .await
        .unwrap();
    token.into_string()
}

async fn create_entity(pool: &PgPool, admin: &str, body: Value) -> EntityDefinition {
    let (status, body) = call(pool, Method::POST, "/api/v1/entities", admin, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value::<EntityDetail>(body).unwrap().entity
}

async fn get_entity(pool: &PgPool, admin: &str, key: &str) -> EntityDetail {
    let (status, body) = call(
        pool,
        Method::GET,
        &format!("/api/v1/entities/{key}"),
        admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn create_field_raw(
    pool: &PgPool,
    admin: &str,
    entity: &str,
    field: Value,
) -> (StatusCode, Value) {
    call(
        pool,
        Method::POST,
        &format!("/api/v1/entities/{entity}/fields"),
        admin,
        Some(field),
    )
    .await
}

async fn create_field(pool: &PgPool, admin: &str, entity: &str, field: Value) -> FieldDefinition {
    let (status, body) = create_field_raw(pool, admin, entity, field).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn field_of(pool: &PgPool, admin: &str, entity: &str, key: &str) -> FieldDefinition {
    get_entity(pool, admin, entity)
        .await
        .fields
        .into_iter()
        .find(|f| f.field_key == key)
        .unwrap()
}

/// 公式欄位的 request。`decimal` / `money` 結果時小數位數 2。
fn formula_field(key: &str, formula: &str, result: &str) -> Value {
    let mut body = json!({
        "field_key": key,
        "label": format!("{key} 公式"),
        "data_type": "formula",
        "formula": formula,
        "formula_result_type": result,
    });
    if matches!(result, "decimal" | "money") {
        body["decimal_places"] = json!(2);
    }
    body
}

/// 以目前的定義為基礎、套用 `changes` 的修改 request（多餘的屬性會被忽略）。
async fn patch_field_raw(
    pool: &PgPool,
    admin: &str,
    entity: &str,
    key: &str,
    changes: Value,
) -> (StatusCode, Value) {
    let current = field_of(pool, admin, entity, key).await;
    let mut body = serde_json::to_value(&current).unwrap();
    for (k, v) in changes.as_object().unwrap() {
        body[k] = v.clone();
    }
    call(
        pool,
        Method::PATCH,
        &format!("/api/v1/entities/{entity}/fields/{key}"),
        admin,
        Some(body),
    )
    .await
}

async fn deactivate_field(
    pool: &PgPool,
    admin: &str,
    entity: &str,
    key: &str,
) -> (StatusCode, Value) {
    let current = field_of(pool, admin, entity, key).await;
    call(
        pool,
        Method::POST,
        &format!("/api/v1/entities/{entity}/fields/{key}/deactivate"),
        admin,
        Some(json!({ "version": current.version })),
    )
    .await
}

async fn patch_entity_active(
    pool: &PgPool,
    admin: &str,
    key: &str,
    is_active: bool,
) -> (StatusCode, Value) {
    let e = get_entity(pool, admin, key).await.entity;
    call(
        pool,
        Method::PATCH,
        &format!("/api/v1/entities/{key}"),
        admin,
        Some(json!({
            "version": e.version,
            "label": e.label,
            "icon": e.icon,
            "numbering_pattern": e.numbering_pattern,
            "title_template": e.title_template,
            "sort_order": e.sort_order,
            "is_active": is_active,
        })),
    )
    .await
}

async fn delete_entity(pool: &PgPool, admin: &str, key: &str) -> (StatusCode, Value) {
    let e = get_entity(pool, admin, key).await.entity;
    call(
        pool,
        Method::DELETE,
        &format!("/api/v1/entities/{key}?version={}", e.version),
        admin,
        None,
    )
    .await
}

async fn formula_deps(pool: &PgPool, field_id: Id) -> Value {
    sqlx::query_scalar::<_, Option<Value>>(
        "SELECT formula_deps FROM field_definitions WHERE id = $1",
    )
    .bind(field_id)
    .fetch_one(pool)
    .await
    .unwrap()
    .unwrap_or(Value::Null)
}

async fn schema_change_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM schema_change_logs")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// 建立以下資料類別，回傳管理者 token。
///
/// | 資料類別 | 欄位 |
/// |---|---|
/// | [`CUSTOMER`] | `name`（text）、`discount`（decimal） |
/// | [`REPAIR`] | `qty`（integer）、`price`（money）、`note`（text）、`customer`（relation → [`CUSTOMER`]） |
/// | [`PARTS`]（[`REPAIR`] 的明細子表） | `amount`（money）、`memo`（text） |
async fn setup(pool: &PgPool) -> String {
    let admin = token_for(pool, "admin", role::ADMIN).await;
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
    let repair = create_entity(
        pool,
        &admin,
        json!({ "key": REPAIR, "label": "維修單", "kind": "document" }),
    )
    .await;
    for f in [
        json!({ "field_key": "qty", "label": "數量", "data_type": "integer" }),
        json!({ "field_key": "price", "label": "單價", "data_type": "money", "decimal_places": 2 }),
        json!({ "field_key": "note", "label": "備註", "data_type": "text" }),
        json!({ "field_key": "customer", "label": "客戶", "data_type": "relation",
                "relation_entity_id": customer.id }),
    ] {
        create_field(pool, &admin, REPAIR, f).await;
    }
    create_entity(
        pool,
        &admin,
        json!({ "key": PARTS, "label": "零件", "kind": "detail", "parent_entity_id": repair.id }),
    )
    .await;
    for f in [
        json!({ "field_key": "amount", "label": "金額", "data_type": "money", "decimal_places": 2 }),
        json!({ "field_key": "memo", "label": "說明", "data_type": "text" }),
    ] {
        create_field(pool, &admin, PARTS, f).await;
    }
    admin
}

/// 建立本筆欄位、關聯引用、子表彙總、明細的公式各一，`formula_deps` 正確；寫入變更紀錄與操作紀錄。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn creates_formula_fields_with_deps(pool: PgPool) {
    let admin = setup(&pool).await;

    let mut f = formula_field("subtotal", " {qty} * {price} ", "money");
    f["is_searchable"] = json!(true);
    let subtotal = create_field(&pool, &admin, REPAIR, f).await;
    assert_eq!(subtotal.formula.as_deref(), Some("{qty} * {price}"));
    assert_eq!(
        serde_json::to_value(subtotal.formula_result_type).unwrap(),
        json!("money")
    );
    assert_eq!(subtotal.decimal_places, Some(2));
    assert!(subtotal.is_searchable);
    assert_eq!(
        formula_deps(&pool, subtotal.id).await,
        json!([
            { "kind": "field", "field": "price" },
            { "kind": "field", "field": "qty" },
        ])
    );

    let discounted = create_field(
        &pool,
        &admin,
        REPAIR,
        formula_field(
            "discounted",
            "ROUND({subtotal} * (1 - {customer.discount}), 2)",
            "money",
        ),
    )
    .await;
    assert_eq!(
        formula_deps(&pool, discounted.id).await,
        json!([
            { "kind": "field", "field": "subtotal" },
            { "kind": "relation", "relation": "customer", "field": "discount" },
        ])
    );

    let parts_total = create_field(
        &pool,
        &admin,
        REPAIR,
        formula_field(
            "parts_total",
            "SUM(parts.amount) + COUNT(parts.memo)",
            "money",
        ),
    )
    .await;
    assert_eq!(
        formula_deps(&pool, parts_total.id).await,
        json!([
            { "kind": "detail", "detail": "parts", "field": "amount" },
            { "kind": "detail", "detail": "parts", "field": "memo" },
        ])
    );

    let double = create_field(
        &pool,
        &admin,
        PARTS,
        formula_field("double", "{amount} * 2", "money"),
    )
    .await;
    assert_eq!(
        formula_deps(&pool, double.id).await,
        json!([{ "kind": "field", "field": "amount" }])
    );
    // 主表彙總明細的公式欄位
    create_field(
        &pool,
        &admin,
        REPAIR,
        formula_field("doubles", "SUM(parts.double)", "money"),
    )
    .await;

    // 文字、日期、布林結果
    for (key, formula, result) in [
        ("label_text", "{note} & \"-\" & {qty}", "text"),
        ("is_big", "{qty} > 10", "boolean"),
        ("due", "TODAY()", "date"),
    ] {
        create_field(&pool, &admin, REPAIR, formula_field(key, formula, result)).await;
    }

    // 有公式欄位的資料類別仍可新增、讀取資料（存檔時計算在之後的 issue）
    let (status, body) = call(
        &pool,
        Method::POST,
        &format!("/api/v1/entities/{REPAIR}/records"),
        &admin,
        Some(json!({ "data": { "qty": 2, "note": "A" }, "lines": { "parts": [{ "data": { "amount": "10" } }] } })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, body) = call(
        &pool,
        Method::GET,
        &format!("/api/v1/entities/{REPAIR}/records"),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], json!(1));

    // 變更紀錄與操作紀錄
    let (action, after): (String, Value) = sqlx::query_as(
        "SELECT action, after_data FROM schema_change_logs
         WHERE target_type = 'field' AND target_id = $1",
    )
    .bind(subtotal.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(action, "create");
    assert_eq!(after["formula"], json!("{qty} * {price}"));
    assert_eq!(after["formula_result_type"], json!("money"));
    let audits: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_logs WHERE entity_key = 'field_definition' AND record_id = $1",
    )
    .bind(subtotal.id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(audits, ["create"]);
}

/// 語法錯誤、引用不存在或已停用的欄位、型態不符、循環引用都回傳 422 並指出原因；不寫入。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn invalid_formulas_are_rejected(pool: PgPool) {
    let admin = setup(&pool).await;
    // 已停用的欄位、關聯目標的已停用欄位、已停用的明細子表
    create_field(
        &pool,
        &admin,
        REPAIR,
        json!({ "field_key": "old", "label": "舊欄位", "data_type": "integer" }),
    )
    .await;
    let (status, body) = deactivate_field(&pool, &admin, REPAIR, "old").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = deactivate_field(&pool, &admin, CUSTOMER, "name").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let repair = get_entity(&pool, &admin, REPAIR).await.entity;
    create_entity(
        &pool,
        &admin,
        json!({ "key": "labor", "label": "工時", "kind": "detail", "parent_entity_id": repair.id }),
    )
    .await;
    create_field(
        &pool,
        &admin,
        "labor",
        json!({ "field_key": "hours", "label": "時數", "data_type": "integer" }),
    )
    .await;
    let (status, body) = patch_entity_active(&pool, &admin, "labor", false).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let logs_before = schema_change_count(&pool).await;
    let cases: &[(&str, &str, (u64, u64), &str)] = &[
        // 語法
        ("{qty} *", "integer", (7, 7), "公式不完整"),
        ("{qty} + )", "integer", (8, 9), "「)」"),
        // 引用不存在、已停用的欄位
        ("{missing} + 1", "integer", (0, 9), "找不到欄位「missing」"),
        ("1 + {old}", "integer", (4, 9), "找不到欄位「old」"),
        (
            "{customer.nope}",
            "text",
            (0, 15),
            "找不到關聯欄位「customer.nope」",
        ),
        (
            "{customer.name}",
            "text",
            (0, 15),
            "找不到關聯欄位「customer.name」",
        ),
        ("{note.name}", "text", (0, 11), "不是關聯欄位"),
        (
            "SUM(parts.nope)",
            "money",
            (0, 15),
            "找不到明細欄位「parts.nope」",
        ),
        (
            "SUM(lines.amount)",
            "money",
            (0, 17),
            "找不到明細子表「lines」",
        ),
        (
            "SUM(labor.hours)",
            "integer",
            (0, 16),
            "找不到明細子表「labor」",
        ),
        // 型態不符
        ("{note} * 2", "integer", (0, 6), "需要"),
        ("{qty} + 1", "text", (0, 9), "無法存成 text"),
        ("SUM(parts.memo)", "money", (0, 15), "只能用於數字欄位"),
        (
            "IF({qty} > 1, 1, \"a\")",
            "integer",
            (17, 20),
            "結果型態必須一致",
        ),
    ];
    for (formula, result, span, message) in cases {
        let (status, body) =
            create_field_raw(&pool, &admin, REPAIR, formula_field("bad", formula, result)).await;
        assert_formula_span(status, &body, *span, message);
    }

    // 明細的公式不能引用主表欄位、不能使用子表彙總
    let (status, body) = create_field_raw(
        &pool,
        &admin,
        PARTS,
        formula_field("bad", "{qty}", "integer"),
    )
    .await;
    assert_formula_span(status, &body, (0, 5), "主表欄位");
    let (status, body) = create_field_raw(
        &pool,
        &admin,
        PARTS,
        formula_field("bad", "SUM(parts.amount)", "money"),
    )
    .await;
    assert_formula_span(status, &body, (0, 17), "子表彙總");

    // 引用自己
    let (status, body) = create_field_raw(
        &pool,
        &admin,
        REPAIR,
        formula_field("me", "{me} + 1", "integer"),
    )
    .await;
    let details = validation_details(status, &body);
    assert_eq!(details["field"], json!("formula"));
    assert_eq!(details["cycle"], json!(["me", "me"]));
    assert!(body["message"].as_str().unwrap().contains("循環引用"));

    // 公式與結果型態必填、結果型態不支援
    let mut no_formula = formula_field("bad", "1", "integer");
    no_formula.as_object_mut().unwrap().remove("formula");
    let (status, body) = create_field_raw(&pool, &admin, REPAIR, no_formula).await;
    assert_invalid(status, &body, "formula");
    let mut no_result = formula_field("bad", "1", "integer");
    no_result
        .as_object_mut()
        .unwrap()
        .remove("formula_result_type");
    let (status, body) = create_field_raw(&pool, &admin, REPAIR, no_result).await;
    assert_invalid(status, &body, "formula_result_type");
    let (status, body) =
        create_field_raw(&pool, &admin, REPAIR, formula_field("bad", "1", "select")).await;
    assert_invalid(status, &body, "formula_result_type");
    // decimal / money 結果需要小數位數
    let mut no_places = formula_field("bad", "1", "decimal");
    no_places.as_object_mut().unwrap().remove("decimal_places");
    let (status, body) = create_field_raw(&pool, &admin, REPAIR, no_places).await;
    assert_invalid(status, &body, "decimal_places");
    // 其他型態不可設定公式
    let (status, body) = create_field_raw(
        &pool,
        &admin,
        REPAIR,
        json!({ "field_key": "bad", "label": "x", "data_type": "text", "formula": "1" }),
    )
    .await;
    assert_invalid(status, &body, "formula");

    assert_eq!(schema_change_count(&pool).await, logs_before);
    assert!(
        get_entity(&pool, &admin, REPAIR)
            .await
            .fields
            .iter()
            .all(|f| f.field_key != "bad" && f.field_key != "me")
    );
}

/// 修改公式造成循環引用時拒絕，回傳循環路徑；合法的修改更新 `formula_deps` 並留紀錄。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn update_formula_checks_cycles(pool: PgPool) {
    let admin = setup(&pool).await;
    let a = create_field(
        &pool,
        &admin,
        REPAIR,
        formula_field("a", "{qty} + 1", "integer"),
    )
    .await;
    create_field(
        &pool,
        &admin,
        REPAIR,
        formula_field("b", "{a} * 2", "integer"),
    )
    .await;
    create_field(
        &pool,
        &admin,
        REPAIR,
        formula_field("c", "{b} - 1", "integer"),
    )
    .await;

    let (status, body) =
        patch_field_raw(&pool, &admin, REPAIR, "a", json!({ "formula": "{c} + 1" })).await;
    let details = validation_details(status, &body);
    assert_eq!(details["field"], json!("formula"));
    assert_eq!(details["cycle"], json!(["a", "c", "b", "a"]));
    assert!(
        body["message"].as_str().unwrap().contains("a → c → b → a"),
        "{body}"
    );
    // 明細的公式之間也檢查
    create_field(
        &pool,
        &admin,
        PARTS,
        formula_field("x", "{amount} + 1", "money"),
    )
    .await;
    create_field(&pool, &admin, PARTS, formula_field("y", "{x} + 1", "money")).await;
    let (status, body) =
        patch_field_raw(&pool, &admin, PARTS, "x", json!({ "formula": "{y}" })).await;
    let details = validation_details(status, &body);
    assert_eq!(details["cycle"], json!(["parts.x", "parts.y", "parts.x"]));

    // 合法的修改：公式與結果型態都可以修改
    let logs_before = schema_change_count(&pool).await;
    let (status, body) = patch_field_raw(
        &pool,
        &admin,
        REPAIR,
        "a",
        json!({ "formula": "{note} & {customer.name}", "formula_result_type": "text" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let updated: FieldDefinition = serde_json::from_value(body).unwrap();
    assert_eq!(updated.formula.as_deref(), Some("{note} & {customer.name}"));
    assert_eq!(
        formula_deps(&pool, a.id).await,
        json!([
            { "kind": "field", "field": "note" },
            { "kind": "relation", "relation": "customer", "field": "name" },
        ])
    );
    assert_eq!(schema_change_count(&pool).await, logs_before + 1);
    let (action, before, after): (String, Value, Value) = sqlx::query_as(
        "SELECT action, before_data, after_data FROM schema_change_logs
         WHERE target_type = 'field' AND target_id = $1 ORDER BY changed_at DESC, id DESC LIMIT 1",
    )
    .bind(a.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(action, "update");
    assert_eq!(before["formula"], json!("{qty} + 1"));
    assert_eq!(after["formula_result_type"], json!("text"));
    let (audit_action, audit_after): (String, Value) = sqlx::query_as(
        "SELECT action, after_data FROM audit_logs
         WHERE entity_key = 'field_definition' AND record_id = $1
         ORDER BY occurred_at DESC, id DESC LIMIT 1",
    )
    .bind(a.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audit_action, "update");
    assert_eq!(audit_after["formula"], json!("{note} & {customer.name}"));
    assert_eq!(audit_after["formula_result_type"], json!("text"));

    // 修改只有顯示名稱時不重新檢查公式（formula_deps 不變）
    let (status, body) =
        patch_field_raw(&pool, &admin, REPAIR, "a", json!({ "label": "新名稱" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        formula_deps(&pool, a.id).await,
        json!([
            { "kind": "field", "field": "note" },
            { "kind": "relation", "relation": "customer", "field": "name" },
        ])
    );
}

/// 公式欄位不可設定「不可重複」、預設值、必填；可以設定「可搜尋」。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn formula_field_restrictions(pool: PgPool) {
    let admin = setup(&pool).await;
    for (prop, value) in [
        ("is_unique", json!(true)),
        ("default_value", json!(1)),
        ("is_required", json!(true)),
    ] {
        let mut f = formula_field("total", "{qty} + 1", "integer");
        f[prop] = value.clone();
        let (status, body) = create_field_raw(&pool, &admin, REPAIR, f).await;
        assert_invalid(status, &body, prop);
    }

    create_field(
        &pool,
        &admin,
        REPAIR,
        formula_field("total", "{qty} + 1", "integer"),
    )
    .await;
    for (prop, value) in [
        ("is_unique", json!(true)),
        ("default_value", json!(1)),
        ("is_required", json!(true)),
    ] {
        let (status, body) =
            patch_field_raw(&pool, &admin, REPAIR, "total", json!({ prop: value })).await;
        assert_invalid(status, &body, prop);
    }
    let (status, body) = patch_field_raw(
        &pool,
        &admin,
        REPAIR,
        "total",
        json!({ "is_searchable": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// 其他型態 ↔ `formula` 的型態變更被拒絕。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn type_change_to_or_from_formula_is_rejected(pool: PgPool) {
    let admin = setup(&pool).await;
    create_field(
        &pool,
        &admin,
        REPAIR,
        formula_field("total", "{qty} + 1", "integer"),
    )
    .await;
    let change = |key: &'static str, body: Value| {
        let pool = pool.clone();
        let admin = admin.clone();
        async move {
            let current = field_of(&pool, &admin, REPAIR, key).await;
            let mut body = body;
            body["version"] = json!(current.version);
            call(
                &pool,
                Method::POST,
                &format!("/api/v1/entities/{REPAIR}/fields/{key}/change-type"),
                &admin,
                Some(body),
            )
            .await
        }
    };
    let (status, body) = change("note", json!({ "data_type": "formula" })).await;
    assert_invalid(status, &body, "data_type");
    for to in ["text", "long_text", "integer"] {
        let (status, body) = change("total", json!({ "data_type": to })).await;
        assert_invalid(status, &body, "data_type");
    }
    // 修改欄位時也不能變更型態
    let (status, body) = patch_field_raw(
        &pool,
        &admin,
        REPAIR,
        "note",
        json!({ "data_type": "formula" }),
    )
    .await;
    assert_invalid(status, &body, "data_type");
}

/// 被有效公式引用的欄位不能停用；停用明細子表、刪除資料類別時也檢查。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn referenced_fields_cannot_be_deactivated(pool: PgPool) {
    let admin = setup(&pool).await;
    for (entity, f) in [
        (
            REPAIR,
            formula_field("subtotal", "{qty} * {price}", "money"),
        ),
        (
            REPAIR,
            formula_field("discounted", "{subtotal} * {customer.discount}", "money"),
        ),
        (
            REPAIR,
            formula_field("parts_total", "SUM(parts.amount)", "money"),
        ),
        (PARTS, formula_field("double", "{amount} * 2", "money")),
    ] {
        create_field(&pool, &admin, entity, f).await;
    }

    // 本筆欄位：停用 API 與修改 API 都拒絕
    let (status, body) = deactivate_field(&pool, &admin, REPAIR, "qty").await;
    assert_referenced_by(status, &body, &[(REPAIR, "subtotal")]);
    let (status, body) =
        patch_field_raw(&pool, &admin, REPAIR, "qty", json!({ "is_active": false })).await;
    assert_referenced_by(status, &body, &[(REPAIR, "subtotal")]);
    // 公式欄位被其他公式引用
    let (status, body) = deactivate_field(&pool, &admin, REPAIR, "subtotal").await;
    assert_referenced_by(status, &body, &[(REPAIR, "discounted")]);
    // 關聯欄位本身、關聯目標的欄位
    let (status, body) = deactivate_field(&pool, &admin, REPAIR, "customer").await;
    assert_referenced_by(status, &body, &[(REPAIR, "discounted")]);
    let (status, body) = deactivate_field(&pool, &admin, CUSTOMER, "discount").await;
    assert_referenced_by(status, &body, &[(REPAIR, "discounted")]);
    // 明細欄位：明細自己的公式與主表的子表彙總，依資料類別、欄位代碼排序
    let (status, body) = deactivate_field(&pool, &admin, PARTS, "amount").await;
    assert_referenced_by(status, &body, &[(PARTS, "double"), (REPAIR, "parts_total")]);
    // 沒有被引用的欄位可以停用
    let (status, body) = deactivate_field(&pool, &admin, REPAIR, "note").await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // 停用、刪除被子表彙總引用的明細子表
    let (status, body) = patch_entity_active(&pool, &admin, PARTS, false).await;
    assert_referenced_by(status, &body, &[(REPAIR, "parts_total")]);
    let (status, body) = delete_entity(&pool, &admin, PARTS).await;
    assert_referenced_by(status, &body, &[(REPAIR, "parts_total")]);

    // 先停用引用的公式，就可以停用
    let (status, body) = deactivate_field(&pool, &admin, REPAIR, "parts_total").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = patch_entity_active(&pool, &admin, PARTS, false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = deactivate_field(&pool, &admin, REPAIR, "discounted").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = deactivate_field(&pool, &admin, CUSTOMER, "discount").await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // 重新啟用公式欄位時重新檢查：引用的明細子表已停用
    let (status, body) = patch_field_raw(
        &pool,
        &admin,
        REPAIR,
        "parts_total",
        json!({ "is_active": true }),
    )
    .await;
    assert_formula_span(status, &body, (0, 17), "找不到明細子表「parts」");

    // 刪除主表：明細子表一起刪除，自己內部的公式不影響
    let (status, body) = delete_entity(&pool, &admin, REPAIR).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
}

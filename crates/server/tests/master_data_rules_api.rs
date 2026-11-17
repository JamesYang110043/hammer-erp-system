//! 基本資料業務規則的整合測試（#107）：代碼、商品、商品分類循環、廠商幣別、統一編號、
//! 使用中的單位與分類不能刪除；錯誤一律 `422 VALIDATION`（docs/architecture.md §7.2、§8.6、§8.7）。
//!
//! 使用 `#[sqlx::test]`，需要 `DATABASE_URL`（見 README「整合測試」）。
//! 透過完整的 router 呼叫，包含所有中介層。

#![allow(clippy::unwrap_used)]

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use hammer_core::test_util::{TestUser, create_user_with_session};
use hammer_server::{AppState, Config, router};
use hammer_shared::record::Record;
use hammer_shared::{ApiError, ErrorCode, Id, role};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

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
    err.details.unwrap()
}

/// 422 VALIDATION，且 `details.fields` 的 `field_key` 依序為 `keys`；回傳第一個欄位的訊息。
fn assert_invalid_fields(status: StatusCode, body: &Value, keys: &[&str]) -> String {
    let details = validation_details(status, body);
    let fields = details["fields"].as_array().unwrap();
    let actual: Vec<&str> = fields
        .iter()
        .map(|f| f["field_key"].as_str().unwrap())
        .collect();
    assert_eq!(actual, keys, "{body}");
    fields[0]["message"].as_str().unwrap().to_owned()
}

async fn editor(pool: &PgPool) -> String {
    let (_, token) = create_user_with_session(pool, &TestUser::new("editor", role::EDITOR))
        .await
        .unwrap();
    token.into_string()
}

fn records_uri(entity: &str) -> String {
    format!("/api/v1/entities/{entity}/records")
}

fn record_uri(entity: &str, id: Id) -> String {
    format!("/api/v1/entities/{entity}/records/{id}")
}

async fn post(pool: &PgPool, token: &str, entity: &str, data: Value) -> (StatusCode, Value) {
    call(
        pool,
        Method::POST,
        &records_uri(entity),
        token,
        Some(json!({ "data": data })),
    )
    .await
}

async fn create(pool: &PgPool, token: &str, entity: &str, data: Value) -> Record {
    let (status, body) = post(pool, token, entity, data).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn patch(
    pool: &PgPool,
    token: &str,
    entity: &str,
    record: &Record,
    data: Value,
) -> (StatusCode, Value) {
    call(
        pool,
        Method::PATCH,
        &record_uri(entity, record.id),
        token,
        Some(json!({ "version": record.version, "data": data })),
    )
    .await
}

async fn delete(pool: &PgPool, token: &str, entity: &str, record: &Record) -> (StatusCode, Value) {
    call(
        pool,
        Method::DELETE,
        &format!(
            "{}?version={}",
            record_uri(entity, record.id),
            record.version
        ),
        token,
        None,
    )
    .await
}

async fn get(pool: &PgPool, token: &str, entity: &str, id: Id) -> Record {
    let (status, body) = call(pool, Method::GET, &record_uri(entity, id), token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn unit(pool: &PgPool, token: &str, code: &str) -> Record {
    create(pool, token, "unit", json!({ "code": code, "name": code })).await
}

async fn category(pool: &PgPool, token: &str, name: &str, parent: Option<Id>) -> Record {
    create(
        pool,
        token,
        "product_category",
        json!({ "name": name, "parent_id": parent }),
    )
    .await
}

fn product_data(code: &str, unit_id: Id) -> Value {
    json!({ "code": code, "name": "螺絲", "unit_id": unit_id })
}

// ---------------------------------------------------------------------------
// 代碼
// ---------------------------------------------------------------------------

/// 各資料類別的 `code` 必填（去除前後空白後）、不分大小寫不可重複。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn code_is_required_and_unique_case_insensitively(pool: PgPool) {
    let token = editor(&pool).await;
    let pcs = unit(&pool, &token, "pcs").await;

    let cases = [
        ("unit", json!({ "name": "箱" })),
        ("product", json!({ "name": "螺絲", "unit_id": pcs.id })),
        ("customer", json!({ "name": "甲公司" })),
        ("supplier", json!({ "name": "乙公司" })),
    ];
    for (entity, data) in &cases {
        let (status, body) = post(&pool, &token, entity, data.clone()).await;
        assert_invalid_fields(status, &body, &["code"]);

        let mut blank = data.clone();
        blank["code"] = json!("   ");
        let (status, body) = post(&pool, &token, entity, blank).await;
        assert_invalid_fields(status, &body, &["code"]);

        let mut first = data.clone();
        first["code"] = json!(" A001 ");
        let created = create(&pool, &token, entity, first).await;
        assert_eq!(created.data["code"], "A001", "{entity}");

        let mut dup = data.clone();
        dup["code"] = json!("a001");
        let (status, body) = post(&pool, &token, entity, dup).await;
        let message = assert_invalid_fields(status, &body, &["code"]);
        assert_eq!(message, "此值已被其他資料使用", "{entity}");
    }
}

// ---------------------------------------------------------------------------
// 商品
// ---------------------------------------------------------------------------

/// 商品的類型、稅別、單位、預設售價、安全庫存。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn product_rules(pool: PgPool) {
    let token = editor(&pool).await;
    let pcs = unit(&pool, &token, "pcs").await;

    let product = create(&pool, &token, "product", product_data("A001", pcs.id)).await;
    assert_eq!(product.data["product_type"], "goods");
    assert_eq!(product.data["tax_type"], "taxable");
    assert_eq!(product.data["unit_id"], json!(pcs.id));

    let invalid = [
        ("product_type", json!("bundle")),
        ("tax_type", json!("vat")),
        ("unit_id", Value::Null),
        ("default_sale_price", json!("-1")),
        ("safety_stock", json!("-0.5")),
    ];
    for (key, value) in invalid {
        let mut data = product_data("A002", pcs.id);
        data[key] = value;
        let (status, body) = post(&pool, &token, "product", data).await;
        assert_invalid_fields(status, &body, &[key]);
    }

    // 有效的值
    let mut data = product_data("A002", pcs.id);
    data["product_type"] = json!("service");
    data["tax_type"] = json!("zero_rated");
    data["default_sale_price"] = json!("0");
    data["safety_stock"] = json!("0");
    create(&pool, &token, "product", data).await;

    // 單位必須存在且未刪除
    let (status, body) = post(&pool, &token, "product", product_data("A003", Id::nil())).await;
    assert_invalid_fields(status, &body, &["unit_id"]);
    let box_unit = unit(&pool, &token, "box").await;
    let (status, body) = delete(&pool, &token, "unit", &box_unit).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (status, body) = post(&pool, &token, "product", product_data("A003", box_unit.id)).await;
    assert_invalid_fields(status, &body, &["unit_id"]);

    // 修改時清空單位
    let (status, body) = patch(
        &pool,
        &token,
        "product",
        &product,
        json!({ "unit_id": null }),
    )
    .await;
    assert_invalid_fields(status, &body, &["unit_id"]);
}

// ---------------------------------------------------------------------------
// 商品分類
// ---------------------------------------------------------------------------

/// 商品分類的上層分類不可指向自己或自己的下層分類，錯誤指出 `parent_id`。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn category_parent_cannot_form_cycle(pool: PgPool) {
    let token = editor(&pool).await;
    let root = category(&pool, &token, "五金", None).await;
    let child = category(&pool, &token, "螺絲", Some(root.id)).await;
    let grandchild = category(&pool, &token, "六角螺絲", Some(child.id)).await;
    let other = category(&pool, &token, "工具", None).await;

    for parent in [root.id, child.id, grandchild.id] {
        let (status, body) = patch(
            &pool,
            &token,
            "product_category",
            &root,
            json!({ "parent_id": parent }),
        )
        .await;
        let message = assert_invalid_fields(status, &body, &["parent_id"]);
        assert_eq!(message, "上層分類不可是自己或自己的下層分類");
    }
    let (status, body) = patch(
        &pool,
        &token,
        "product_category",
        &child,
        json!({ "parent_id": child.id }),
    )
    .await;
    assert_invalid_fields(status, &body, &["parent_id"]);

    // 移到其他分支、改成最上層可以
    let (status, body) = patch(
        &pool,
        &token,
        "product_category",
        &child,
        json!({ "parent_id": other.id }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let child = get(&pool, &token, "product_category", child.id).await;
    assert_eq!(child.data["parent_id"], json!(other.id));
    let (status, body) = patch(
        &pool,
        &token,
        "product_category",
        &root,
        json!({ "parent_id": grandchild.id }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = patch(
        &pool,
        &token,
        "product_category",
        &child,
        json!({ "parent_id": null }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // 上層分類必須存在
    let (status, body) = post(
        &pool,
        &token,
        "product_category",
        json!({ "name": "x", "parent_id": Id::nil() }),
    )
    .await;
    assert_invalid_fields(status, &body, &["parent_id"]);
}

// ---------------------------------------------------------------------------
// 客戶與廠商
// ---------------------------------------------------------------------------

/// 廠商的預設幣別必須是有效的幣別；修改時只在值有變動時檢查。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn supplier_currency_must_be_active(pool: PgPool) {
    let token = editor(&pool).await;
    let twd = create(
        &pool,
        &token,
        "supplier",
        json!({ "code": "S001", "name": "甲" }),
    )
    .await;
    assert_eq!(twd.data["default_currency_code"], "TWD");
    let usd = create(
        &pool,
        &token,
        "supplier",
        json!({ "code": "S002", "name": "乙", "default_currency_code": "USD" }),
    )
    .await;

    for code in [json!("XXX"), json!("usd"), Value::Null] {
        let (status, body) = post(
            &pool,
            &token,
            "supplier",
            json!({ "code": "S003", "name": "丙", "default_currency_code": code }),
        )
        .await;
        assert_invalid_fields(status, &body, &["default_currency_code"]);
    }

    sqlx::query("UPDATE currencies SET is_active = false WHERE code = 'USD'")
        .execute(&pool)
        .await
        .unwrap();
    let (status, body) = patch(
        &pool,
        &token,
        "supplier",
        &twd,
        json!({ "default_currency_code": "USD" }),
    )
    .await;
    let message = assert_invalid_fields(status, &body, &["default_currency_code"]);
    assert_eq!(message, "不是有效的幣別");
    // 幣別停用後，既有資料保留原值、可以修改其他欄位
    let (status, body) = patch(&pool, &token, "supplier", &usd, json!({ "name": "乙公司" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// 客戶、廠商的統一編號有填時必須是 8 位數字；付款天數、信用額度不可為負數。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn customer_and_supplier_rules(pool: PgPool) {
    let token = editor(&pool).await;
    for entity in ["customer", "supplier"] {
        let record = create(
            &pool,
            &token,
            entity,
            json!({ "code": "C001", "name": "甲", "tax_id": "04595257" }),
        )
        .await;
        // 國外客戶沒有統一編號
        create(
            &pool,
            &token,
            entity,
            json!({ "code": "C002", "name": "乙" }),
        )
        .await;

        for bad in ["1234567", "123456789", "A1234567"] {
            let (status, body) = post(
                &pool,
                &token,
                entity,
                json!({ "code": "C003", "name": "丙", "tax_id": bad }),
            )
            .await;
            let message = assert_invalid_fields(status, &body, &["tax_id"]);
            assert_eq!(message, "統一編號必須是 8 位數字");
        }
        let (status, body) =
            patch(&pool, &token, entity, &record, json!({ "tax_id": "123" })).await;
        assert_invalid_fields(status, &body, &["tax_id"]);

        let (status, body) = post(
            &pool,
            &token,
            entity,
            json!({ "code": "C003", "name": "丙", "payment_terms_days": -1 }),
        )
        .await;
        assert_invalid_fields(status, &body, &["payment_terms_days"]);
        let (status, body) = post(
            &pool,
            &token,
            entity,
            json!({ "code": "C003", "name": "丙", "credit_limit": "-0.01" }),
        )
        .await;
        assert_invalid_fields(status, &body, &["credit_limit"]);
    }
}

// ---------------------------------------------------------------------------
// 刪除
// ---------------------------------------------------------------------------

/// 還有未刪除商品使用的單位、分類不能刪除，回傳使用中的商品數；有子分類的分類不能刪除。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn units_and_categories_in_use_cannot_be_deleted(pool: PgPool) {
    let token = editor(&pool).await;
    let pcs = unit(&pool, &token, "pcs").await;
    let root = category(&pool, &token, "五金", None).await;
    let child = category(&pool, &token, "螺絲", Some(root.id)).await;
    let mut products = Vec::new();
    for code in ["A001", "A002"] {
        let mut data = product_data(code, pcs.id);
        data["category_id"] = json!(child.id);
        products.push(create(&pool, &token, "product", data).await);
    }

    let (status, body) = delete(&pool, &token, "unit", &pcs).await;
    assert_eq!(
        validation_details(status, &body),
        json!({ "fields": [], "in_use_count": 2 })
    );
    assert_eq!(body["message"], "此單位還有 2 筆商品使用中，無法刪除");

    let (status, body) = delete(&pool, &token, "product_category", &child).await;
    assert_eq!(
        validation_details(status, &body),
        json!({ "fields": [], "in_use_count": 2 })
    );
    let (status, body) = delete(&pool, &token, "product_category", &root).await;
    assert_eq!(
        validation_details(status, &body),
        json!({ "fields": [], "child_count": 1 })
    );
    assert_eq!(body["message"], "此分類還有 1 個子分類，無法刪除");

    // 已刪除的商品不算使用中
    for product in &products {
        let (status, body) = delete(&pool, &token, "product", product).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    }
    for (entity, record) in [
        ("unit", &pcs),
        ("product_category", &child),
        ("product_category", &root),
    ] {
        let (status, body) = delete(&pool, &token, entity, record).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{entity} {body}");
    }
}

//! 系統資料類別的公式欄位整合測試（#109，docs/architecture.md §7.1、§7.6）。
//!
//! - 商品新增公式欄位，引用系統欄位（`default_sale_price`）：存檔時計算，值寫入 `custom_data`、系統欄位
//!   不變；可排序、篩選；金額以 Decimal 計算
//! - 計算錯誤時值為空，原因寫入該資料表的 `formula_errors`
//! - 公式的關聯引用讀取系統資料類別的目標（商品的 `unit_id` → 單位）
//! - 客戶的公式以子表彙總讀取聯絡人（系統明細子表）的數值欄位與公式欄位
//! - 重新計算處理系統資料表與系統明細子表：筆數正確，有變動的資料 `version` + 1，沒有變動的不寫入；
//!   已刪除的資料不處理；系統明細子表 → 404
//!
//! 使用 `#[sqlx::test]`，需要 `DATABASE_URL`（見 README「整合測試」）。

#![allow(clippy::unwrap_used)]

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use hammer_core::test_util::{TestUser, create_user_with_session};
use hammer_server::{AppState, Config, router};
use hammer_shared::pagination::Page;
use hammer_shared::record::Record;
use hammer_shared::schema::{EntityDetail, FieldDefinition, RecalculateResult};
use hammer_shared::{Id, audit, role};
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

async fn admin_token(pool: &PgPool) -> String {
    let (_, token) = create_user_with_session(pool, &TestUser::new("admin", role::ADMIN))
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

async fn create_field(pool: &PgPool, token: &str, entity: &str, field: Value) {
    let (status, body) = call(
        pool,
        Method::POST,
        &format!("/api/v1/entities/{entity}/fields"),
        token,
        Some(field),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

/// 公式欄位的 request。`money` / `decimal` 結果時小數位數為 `places`。
fn formula_field(key: &str, formula: &str, result: &str, places: Option<u8>) -> Value {
    let mut body = json!({
        "field_key": key,
        "label": format!("{key} 公式"),
        "data_type": "formula",
        "formula": formula,
        "formula_result_type": result,
    });
    if let Some(places) = places {
        body["decimal_places"] = json!(places);
    }
    body
}

/// 修改公式欄位的公式（其他屬性維持原值）。
async fn change_formula(pool: &PgPool, token: &str, entity: &str, key: &str, formula: &str) {
    let (status, body) = call(
        pool,
        Method::GET,
        &format!("/api/v1/entities/{entity}"),
        token,
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
        token,
        Some(req),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

async fn create(pool: &PgPool, token: &str, entity: &str, body: Value) -> Record {
    let (status, body) = call(pool, Method::POST, &records_uri(entity), token, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn update(pool: &PgPool, token: &str, entity: &str, id: Id, body: Value) -> Record {
    let (status, body) = call(
        pool,
        Method::PATCH,
        &record_uri(entity, id),
        token,
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn get(pool: &PgPool, token: &str, entity: &str, id: Id) -> Record {
    let (status, body) = call(pool, Method::GET, &record_uri(entity, id), token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn recalculate(pool: &PgPool, token: &str, entity: &str) -> RecalculateResult {
    let (status, body) = call(
        pool,
        Method::POST,
        &format!("/api/v1/entities/{entity}/recalculate"),
        token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

/// 列表（依 `params`）的商品代碼。
async fn product_codes(pool: &PgPool, token: &str, params: &[(&str, &str)]) -> Vec<String> {
    let query: Vec<String> = params.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let (status, body) = call(
        pool,
        Method::GET,
        &format!("{}?{}", records_uri("product"), query.join("&")),
        token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let page: Page<Record> = serde_json::from_value(body).unwrap();
    page.items
        .iter()
        .map(|r| r.data["code"].as_str().unwrap().to_owned())
        .collect()
}

/// 欄位的值；沒有值時為 `null`。
fn value(record: &Record, key: &str) -> Value {
    record.data.get(key).cloned().unwrap_or(Value::Null)
}

/// 資料表中一筆資料的 `custom_data`、`formula_errors` 與 `version`。
async fn stored(pool: &PgPool, table: &str, id: Id) -> (Value, Option<Value>, i32) {
    let sql = match table {
        "products" => "SELECT custom_data, formula_errors, version FROM products WHERE id = $1",
        "customers" => "SELECT custom_data, formula_errors, version FROM customers WHERE id = $1",
        "customer_contacts" => {
            "SELECT custom_data, formula_errors, version FROM customer_contacts WHERE id = $1"
        }
        _ => panic!("未支援的資料表 {table}"),
    };
    sqlx::query_as(sql).bind(id).fetch_one(pool).await.unwrap()
}

async fn unit(pool: &PgPool, token: &str, code: &str, name: &str) -> Record {
    create(
        pool,
        token,
        "unit",
        json!({ "data": { "code": code, "name": name } }),
    )
    .await
}

fn product(code: &str, unit_id: Id, price: Option<&str>) -> Value {
    let mut data = json!({ "code": code, "name": format!("商品{code}"), "unit_id": unit_id });
    if let Some(price) = price {
        data["default_sale_price"] = json!(price);
    }
    json!({ "data": data })
}

// ---------------------------------------------------------------------------
// 存檔時計算
// ---------------------------------------------------------------------------

/// 商品的公式欄位引用系統欄位：存檔後值正確（Decimal），值存在 `custom_data`、系統欄位不變；
/// 可依公式欄位排序、篩選；修改系統欄位後重算。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn product_formula_uses_system_fields(pool: PgPool) {
    let admin = admin_token(&pool).await;
    let pcs = unit(&pool, &admin, "pcs", "個").await;
    create_field(
        &pool,
        &admin,
        "product",
        formula_field("with_tax", "{default_sale_price} * 1.05", "money", Some(4)),
    )
    .await;
    create_field(
        &pool,
        &admin,
        "product",
        formula_field("plus", "{default_sale_price} + 0.2", "money", Some(4)),
    )
    .await;

    let a = create(&pool, &admin, "product", product("A", pcs.id, Some("100"))).await;
    let b = create(&pool, &admin, "product", product("B", pcs.id, Some("0.1"))).await;
    let c = create(&pool, &admin, "product", product("C", pcs.id, Some("50"))).await;
    let none = create(&pool, &admin, "product", product("N", pcs.id, None)).await;

    assert_eq!(value(&a, "default_sale_price"), json!("100.0000"));
    assert_eq!(value(&a, "with_tax"), json!("105.0000"));
    // Decimal：0.1 × 1.05 = 0.105、0.1 + 0.2 = 0.3（浮點數會有誤差）
    assert_eq!(value(&b, "with_tax"), json!("0.1050"));
    assert_eq!(value(&b, "plus"), json!("0.3000"));
    assert!(b.formula_errors.is_empty());
    // 引用的欄位沒有值：結果為空值，沒有錯誤
    assert_eq!(value(&none, "with_tax"), Value::Null);
    assert!(none.formula_errors.is_empty());

    // 公式的值存在 custom_data，系統欄位仍在實體欄位
    let (custom, errors, _) = stored(&pool, "products", a.id).await;
    assert_eq!(
        custom,
        json!({ "with_tax": "105.0000", "plus": "100.2000" })
    );
    assert_eq!(errors, None);
    let price: String =
        sqlx::query_scalar("SELECT default_sale_price::text FROM products WHERE id = $1")
            .bind(a.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(price, "100.0000");

    // 讀取時同樣帶公式的值
    assert_eq!(
        value(&get(&pool, &admin, "product", c.id).await, "with_tax"),
        json!("52.5000")
    );

    // 排序與篩選依結果型態（數值比較，不是字串）
    assert_eq!(
        product_codes(&pool, &admin, &[("sort", "with_tax")]).await,
        ["B", "C", "A", "N"]
    );
    assert_eq!(
        product_codes(&pool, &admin, &[("sort", "-with_tax")]).await,
        ["A", "C", "B", "N"]
    );
    assert_eq!(
        product_codes(
            &pool,
            &admin,
            &[("filter.with_tax.gte", "52.5"), ("sort", "code")]
        )
        .await,
        ["A", "C"]
    );
    assert_eq!(
        product_codes(&pool, &admin, &[("filter.with_tax.empty", "true")]).await,
        ["N"]
    );

    // 修改系統欄位後，公式依新值計算
    let a2 = update(
        &pool,
        &admin,
        "product",
        a.id,
        json!({ "version": a.version, "data": { "default_sale_price": "200" } }),
    )
    .await;
    assert_eq!(value(&a2, "with_tax"), json!("210.0000"));
    assert_eq!(a2.version, a.version + 1);
}

/// 計算錯誤時值為空，原因在回應與資料表的 `formula_errors`；改正後錯誤清除。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn product_formula_errors_are_stored(pool: PgPool) {
    let admin = admin_token(&pool).await;
    let pcs = unit(&pool, &admin, "pcs", "個").await;
    create_field(
        &pool,
        &admin,
        "product",
        formula_field(
            "per_stock",
            "{default_sale_price} / {safety_stock}",
            "decimal",
            Some(2),
        ),
    )
    .await;

    let mut body = product("A", pcs.id, Some("10"));
    body["data"]["safety_stock"] = json!("0");
    let a = create(&pool, &admin, "product", body).await;
    assert_eq!(value(&a, "per_stock"), Value::Null);
    assert_eq!(
        a.formula_errors.get("per_stock").map(String::as_str),
        Some("除數不可為零")
    );
    let (custom, errors, _) = stored(&pool, "products", a.id).await;
    assert_eq!(custom, json!({}));
    assert_eq!(errors, Some(json!({ "per_stock": "除數不可為零" })));

    let fixed = update(
        &pool,
        &admin,
        "product",
        a.id,
        json!({ "version": a.version, "data": { "safety_stock": "4" } }),
    )
    .await;
    assert_eq!(value(&fixed, "per_stock"), json!("2.50"));
    assert!(fixed.formula_errors.is_empty());
    let (custom, errors, _) = stored(&pool, "products", a.id).await;
    assert_eq!(custom, json!({ "per_stock": "2.50" }));
    assert_eq!(errors, None);
}

/// 公式的關聯引用可以讀取系統資料類別的目標（商品的 `unit_id` → 單位的系統欄位）。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn relation_reference_reads_system_target(pool: PgPool) {
    let admin = admin_token(&pool).await;
    let pcs = unit(&pool, &admin, "pcs", "個").await;
    create_field(
        &pool,
        &admin,
        "product",
        formula_field("label", "{name} & \" / \" & {unit_id.name}", "text", None),
    )
    .await;
    let a = create(&pool, &admin, "product", product("A", pcs.id, None)).await;
    assert_eq!(value(&a, "label"), json!("商品A / 個"));
    assert!(a.formula_errors.is_empty());

    // 目標修改後不自動更新；重新計算後更新
    update(
        &pool,
        &admin,
        "unit",
        pcs.id,
        json!({ "version": pcs.version, "data": { "name": "件" } }),
    )
    .await;
    assert_eq!(
        value(&get(&pool, &admin, "product", a.id).await, "label"),
        json!("商品A / 個")
    );
    let result = recalculate(&pool, &admin, "product").await;
    assert_eq!(
        result,
        RecalculateResult {
            processed: 1,
            changed: 1,
            with_errors: 0,
        }
    );
    let a2 = get(&pool, &admin, "product", a.id).await;
    assert_eq!(value(&a2, "label"), json!("商品A / 件"));
    assert_eq!(a2.version, a.version + 1);
}

// ---------------------------------------------------------------------------
// 客戶與聯絡人（系統明細子表）
// ---------------------------------------------------------------------------

/// 聯絡人的 `rate`（money）與公式 `bonus = {rate} * 2`；客戶的
/// `rate_total = SUM(customer_contact.rate)`、`bonus_total = SUM(customer_contact.bonus)`、
/// `limit_ratio = {credit_limit} / 100`。
async fn setup_customer_formulas(pool: &PgPool, admin: &str) {
    create_field(
        pool,
        admin,
        "customer_contact",
        json!({ "field_key": "rate", "label": "費率", "data_type": "money", "decimal_places": 2 }),
    )
    .await;
    create_field(
        pool,
        admin,
        "customer_contact",
        formula_field("bonus", "{rate} * 2", "money", Some(2)),
    )
    .await;
    for (key, formula) in [
        ("rate_total", "SUM(customer_contact.rate)"),
        ("bonus_total", "SUM(customer_contact.bonus)"),
        ("limit_ratio", "{credit_limit} / 100"),
    ] {
        create_field(
            pool,
            admin,
            "customer",
            formula_field(key, formula, "money", Some(2)),
        )
        .await;
    }
}

fn contacts(record: &Record) -> &[Record] {
    &record.lines.as_ref().unwrap()["customer_contact"]
}

/// 客戶的公式以子表彙總讀取聯絡人的數值欄位與公式欄位；聯絡人公式的值存在聯絡人的 `custom_data`。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn customer_aggregates_contact_fields(pool: PgPool) {
    let admin = admin_token(&pool).await;
    setup_customer_formulas(&pool, &admin).await;

    let c = create(
        &pool,
        &admin,
        "customer",
        json!({
            "data": { "code": "C1", "name": "甲公司", "credit_limit": "1000" },
            "lines": { "customer_contact": [
                { "data": { "name": "王小明", "rate": "0.10" } },
                { "data": { "name": "李小華", "rate": "0.20" } },
            ] },
        }),
    )
    .await;
    assert_eq!(value(&c, "rate_total"), json!("0.30"));
    assert_eq!(value(&c, "bonus_total"), json!("0.60"));
    assert_eq!(value(&c, "limit_ratio"), json!("10.00"));
    let bonuses: Vec<Value> = contacts(&c).iter().map(|l| value(l, "bonus")).collect();
    assert_eq!(bonuses, [json!("0.20"), json!("0.40")]);

    let (custom, errors, _) = stored(&pool, "customers", c.id).await;
    assert_eq!(
        custom,
        json!({ "rate_total": "0.30", "bonus_total": "0.60", "limit_ratio": "10.00" })
    );
    assert_eq!(errors, None);
    let (custom, _, _) = stored(&pool, "customer_contacts", contacts(&c)[0].id).await;
    assert_eq!(custom, json!({ "rate": "0.10", "bonus": "0.20" }));
}

/// 重新計算客戶：聯絡人的公式一起更新，客戶的彙總使用新值；有變動的客戶與聯絡人 `version` + 1，
/// 沒有變動的不寫入；已刪除的不處理；再執行一次沒有變動；寫一筆操作紀錄。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn recalculate_customers_and_contacts(pool: PgPool) {
    let admin = admin_token(&pool).await;
    setup_customer_formulas(&pool, &admin).await;

    let with_contacts = create(
        &pool,
        &admin,
        "customer",
        json!({
            "data": { "code": "C1", "name": "甲公司" },
            "lines": { "customer_contact": [
                { "data": { "name": "王小明", "rate": "1" } },
                { "data": { "name": "李小華" } },
            ] },
        }),
    )
    .await;
    let plain = create(
        &pool,
        &admin,
        "customer",
        json!({ "data": { "code": "C2", "name": "乙公司", "credit_limit": "500" } }),
    )
    .await;
    let deleted = create(
        &pool,
        &admin,
        "customer",
        json!({
            "data": { "code": "C3", "name": "丙公司" },
            "lines": { "customer_contact": [ { "data": { "name": "趙", "rate": "1" } } ] },
        }),
    )
    .await;
    let (status, _) = call(
        &pool,
        Method::DELETE,
        &format!(
            "{}?version={}",
            record_uri("customer", deleted.id),
            deleted.version
        ),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // 公式與資料都沒變：沒有寫入
    assert_eq!(
        recalculate(&pool, &admin, "customer").await,
        RecalculateResult {
            processed: 2,
            changed: 0,
            with_errors: 0,
        }
    );
    assert_eq!(
        get(&pool, &admin, "customer", with_contacts.id)
            .await
            .version,
        with_contacts.version
    );

    // 只改聯絡人的公式：有聯絡人的客戶變動，沒有聯絡人的不變
    change_formula(&pool, &admin, "customer_contact", "bonus", "{rate} * 3").await;
    let result = recalculate(&pool, &admin, "customer").await;
    assert_eq!(
        result,
        RecalculateResult {
            processed: 2,
            changed: 1,
            with_errors: 0,
        }
    );
    let c2 = get(&pool, &admin, "customer", with_contacts.id).await;
    assert_eq!(value(&c2, "bonus_total"), json!("3.00"));
    assert_eq!(c2.version, with_contacts.version + 1);
    let before = contacts(&with_contacts);
    let after = contacts(&c2);
    assert_eq!(value(&after[0], "bonus"), json!("3.00"));
    assert_eq!(after[0].version, before[0].version + 1);
    // 第二位聯絡人沒有 rate，bonus 仍為空值：不寫入
    assert_eq!(value(&after[1], "bonus"), Value::Null);
    assert_eq!(after[1].version, before[1].version);
    assert_eq!(
        get(&pool, &admin, "customer", plain.id).await.version,
        plain.version
    );
    // 系統欄位不變
    assert_eq!(value(&c2, "name"), json!("甲公司"));
    assert_eq!(value(&after[0], "name"), json!("王小明"));

    // 已刪除的客戶與其聯絡人不處理
    let (custom, _, version) = stored(&pool, "customers", deleted.id).await;
    assert_eq!(custom["bonus_total"], json!("2.00"));
    assert_eq!(version, deleted.version + 1); // 只有軟刪除時的 + 1
    let (custom, _, _) = stored(&pool, "customer_contacts", contacts(&deleted)[0].id).await;
    assert_eq!(custom["bonus"], json!("2.00"));

    // 計算錯誤的筆數；錯誤寫入 formula_errors
    change_formula(
        &pool,
        &admin,
        "customer",
        "limit_ratio",
        "{credit_limit} / 0",
    )
    .await;
    let result = recalculate(&pool, &admin, "customer").await;
    assert_eq!(
        result,
        RecalculateResult {
            processed: 2,
            changed: 1,
            with_errors: 1,
        }
    );
    let (custom, errors, _) = stored(&pool, "customers", plain.id).await;
    assert_eq!(custom.get("limit_ratio"), None);
    assert_eq!(errors, Some(json!({ "limit_ratio": "除數不可為零" })));

    // 再執行一次沒有變動
    assert_eq!(
        recalculate(&pool, &admin, "customer").await,
        RecalculateResult {
            processed: 2,
            changed: 0,
            with_errors: 1,
        }
    );

    let logs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_logs WHERE action = $1 AND entity_key = 'customer'",
    )
    .bind(audit::RECALCULATE)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(logs, 4);
}

/// 重新計算商品：值有變動的 `version` + 1，回應筆數正確；系統明細子表 → 404。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn recalculate_products(pool: PgPool) {
    let admin = admin_token(&pool).await;
    let pcs = unit(&pool, &admin, "pcs", "個").await;
    create_field(
        &pool,
        &admin,
        "product",
        formula_field("with_tax", "{default_sale_price} * 1.05", "money", Some(2)),
    )
    .await;
    let a = create(&pool, &admin, "product", product("A", pcs.id, Some("100"))).await;
    let b = create(&pool, &admin, "product", product("B", pcs.id, None)).await;

    change_formula(
        &pool,
        &admin,
        "product",
        "with_tax",
        "{default_sale_price} * 1.1",
    )
    .await;
    assert_eq!(
        recalculate(&pool, &admin, "product").await,
        RecalculateResult {
            processed: 2,
            changed: 1,
            with_errors: 0,
        }
    );
    let a2 = get(&pool, &admin, "product", a.id).await;
    assert_eq!(value(&a2, "with_tax"), json!("110.00"));
    assert_eq!(a2.version, a.version + 1);
    assert_eq!(value(&a2, "default_sale_price"), json!("100.0000"));
    assert_eq!(get(&pool, &admin, "product", b.id).await.version, b.version);

    // 沒有公式欄位的系統資料類別也可以執行
    assert_eq!(
        recalculate(&pool, &admin, "unit").await,
        RecalculateResult {
            processed: 1,
            changed: 0,
            with_errors: 0,
        }
    );

    let (status, _) = call(
        &pool,
        Method::POST,
        "/api/v1/entities/customer_contact/recalculate",
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

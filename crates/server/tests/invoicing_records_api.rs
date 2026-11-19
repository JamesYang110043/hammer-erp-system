//! M5 發票模組的通用資料 API 整合測試（#183，docs/architecture.md §8.11）：銷項發票、銷貨折讓、進項憑證、
//! 進貨折讓可以新增草稿（含明細、關聯的出貨單與進貨單）並產生單號；期別由單據日期計算；系統維護的欄位
//! request 帶了一律忽略；折讓從原發票（憑證）帶入客戶（廠商）與稅別，建立後不可更換原發票。
//!
//! 使用 `#[sqlx::test]`，需要 `DATABASE_URL`（見 README「整合測試」）。
//! 透過完整的 router 呼叫，包含所有中介層。

#![allow(clippy::unwrap_used)]

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use hammer_core::test_util::{TestUser, create_user_with_session};
use hammer_server::{AppState, Config, router};
use hammer_shared::record::Record;
use hammer_shared::{Id, role};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

fn state(pool: &PgPool) -> AppState {
    let config = Config::from_sources(None, |name| match name {
        "DATABASE_URL" => Some("postgres://unused".to_owned()),
        _ => None,
    })
    .unwrap();
    AppState::new(pool.clone(), config)
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
    let res = router(state(pool))
        .oneshot(req.body(body).unwrap())
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

async fn editor(pool: &PgPool) -> (Id, String) {
    let (user, token) = create_user_with_session(pool, &TestUser::new("editor", role::EDITOR))
        .await
        .unwrap();
    (user, token.into_string())
}

fn records_uri(entity: &str) -> String {
    format!("/api/v1/entities/{entity}/records")
}

async fn create(pool: &PgPool, token: &str, entity: &str, body: Value) -> Record {
    let (status, body) = call(pool, Method::POST, &records_uri(entity), token, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

/// 客戶、廠商與一個商品。
async fn master_data(pool: &PgPool, token: &str) -> (Id, Id, Id) {
    let customer = create(
        pool,
        token,
        "customer",
        json!({ "data": { "code": "C001", "name": "大華公司", "tax_id": "12345678" } }),
    )
    .await
    .id;
    let supplier = create(
        pool,
        token,
        "supplier",
        json!({ "data": { "code": "S001", "name": "永豐材料", "default_currency_code": "TWD" } }),
    )
    .await
    .id;
    let unit = create(
        pool,
        token,
        "unit",
        json!({ "data": { "code": "pcs", "name": "個" } }),
    )
    .await
    .id;
    let product = create(
        pool,
        token,
        "product",
        json!({ "data": { "code": "A001", "name": "商品 A001", "unit_id": unit } }),
    )
    .await
    .id;
    (customer, supplier, product)
}

fn lines<'a>(record: &'a Record, detail: &str) -> &'a [Record] {
    &record.lines.as_ref().unwrap()[detail]
}

/// 銷項發票（含明細、關聯出貨單）與銷貨折讓。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn sales_invoice_and_allowance_drafts(pool: PgPool) {
    let (user, token) = editor(&pool).await;
    let (customer, _, product) = master_data(&pool, &token).await;
    let shipment = create(
        &pool,
        &token,
        "shipment",
        json!({
            "data": { "customer_id": customer, "ship_date": "2026-11-10", "tax_type": "taxable" },
            "lines": { "shipment_line": [
                { "data": { "product_id": product, "qty": "2", "unit_price": "100" } },
            ] },
        }),
    )
    .await;
    // 發票只能關聯已過帳的出貨單（#184）；以 SQL 模擬過帳，不需要庫存
    sqlx::query(
        "UPDATE shipments SET status = 'posted', posted_at = now(), posted_by = $2 WHERE id = $1",
    )
    .bind(shipment.id)
    .bind(user)
    .execute(&pool)
    .await
    .unwrap();

    let invoice = create(
        &pool,
        &token,
        "sales_invoice",
        json!({
            "data": {
                "invoice_no": "AB12345678", "invoice_date": "2026-12-05", "customer_id": customer,
                "buyer_name": "大華公司", "tax_type": "taxable",
                // 系統維護的欄位：忽略
                "status": "posted", "tax_period": "209901", "total": "999",
                "void_date": "2026-12-06", "void_reason": "x",
            },
            "lines": {
                "sales_invoice_line": [
                    { "data": { "product_id": product, "description": "商品 A001", "qty": "2",
                                "unit_price": "100", "amount": "1" } },
                    { "data": { "description": "運費", "qty": "1", "unit_price": "50" } },
                ],
                "sales_invoice_shipment": [ { "data": { "shipment_id": shipment.id } } ],
            },
        }),
    )
    .await;
    assert!(invoice.record_no.as_deref().unwrap().starts_with("SI20"));
    assert_eq!(invoice.title, "AB12345678 大華公司");
    assert_eq!(invoice.data["status"], "draft");
    assert_eq!(invoice.data["invoice_type"], "triplicate");
    assert_eq!(invoice.data["tax_period"], "202611");
    for key in ["void_date", "void_reason"] {
        assert!(invoice.data.get(key).is_none_or(Value::is_null), "{key}");
    }
    let inv_lines = lines(&invoice, "sales_invoice_line");
    assert_eq!(inv_lines.len(), 2);
    assert_eq!(inv_lines[0].title, "商品 A001");
    assert!(
        inv_lines[1]
            .data
            .get("product_id")
            .is_none_or(Value::is_null)
    );
    let links = lines(&invoice, "sales_invoice_shipment");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].data["shipment_id"], json!(shipment.id));
    assert_eq!(links[0].title, shipment.record_no.clone().unwrap());

    // 修改發票日期 → 期別重新計算
    let uri = format!("{}/{}", records_uri("sales_invoice"), invoice.id);
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &uri,
        &token,
        Some(json!({ "version": invoice.version, "data": { "invoice_date": "2027-01-02" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["tax_period"], "202701");

    let allowance = create(
        &pool,
        &token,
        "sales_allowance",
        json!({
            "data": {
                "sales_invoice_id": invoice.id, "allowance_date": "2027-02-01", "reason": "降價",
                // 系統維護的欄位：忽略，一律帶入發票的值
                "customer_id": Id::now_v7(), "tax_type": "exempt",
            },
            "lines": { "sales_allowance_line": [
                { "data": { "description": "商品 A001 降價", "qty": "1", "unit_price": "10" } },
            ] },
        }),
    )
    .await;
    assert!(allowance.record_no.as_deref().unwrap().starts_with("SA20"));
    assert!(allowance.title.contains("大華公司"), "{}", allowance.title);
    assert_eq!(allowance.data["customer_id"], json!(customer));
    assert_eq!(allowance.data["tax_type"], "taxable");
    assert_eq!(allowance.data["tax_period"], "202701");
    assert_eq!(allowance.data["status"], "draft");
    assert_eq!(lines(&allowance, "sales_allowance_line").len(), 1);

    // 不可更換原發票
    let other = create(
        &pool,
        &token,
        "sales_invoice",
        json!({ "data": { "invoice_no": "AB12345679", "invoice_date": "2027-01-02",
                          "customer_id": customer, "tax_type": "taxable",
                          "invoice_type": "duplicate" } }),
    )
    .await;
    assert_eq!(other.data["invoice_type"], "duplicate");
    let uri = format!("{}/{}", records_uri("sales_allowance"), allowance.id);
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &uri,
        &token,
        Some(json!({ "version": allowance.version, "data": { "sales_invoice_id": other.id } })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([{ "field_key": "sales_invoice_id", "message": "建立後不可更換發票" }])
    );

    // 其他欄位可以修改
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &uri,
        &token,
        Some(json!({ "version": allowance.version, "data": { "allowance_date": "2027-03-01" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["tax_period"], "202703");
    assert_eq!(body["data"]["customer_id"], json!(customer));
}

/// 進項憑證（關聯進貨單）與進貨折讓。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn purchase_invoice_and_allowance_drafts(pool: PgPool) {
    let (_, token) = editor(&pool).await;
    let (_, supplier, product) = master_data(&pool, &token).await;
    let receipt = create(
        &pool,
        &token,
        "goods_receipt",
        json!({
            "data": { "supplier_id": supplier, "receipt_date": "2026-11-10", "currency_code": "TWD",
                      "tax_type": "taxable" },
            "lines": { "goods_receipt_line": [
                { "data": { "product_id": product, "qty": "2", "unit_price": "100" } },
            ] },
        }),
    )
    .await;

    let invoice = create(
        &pool,
        &token,
        "purchase_invoice",
        json!({
            "data": {
                "invoice_no": "CD12345678", "invoice_date": "2026-11-12", "supplier_id": supplier,
                "subtotal": "200", "tax_amount": "10",
                "status": "posted", "tax_period": "x",
            },
            "lines": { "purchase_invoice_receipt": [ { "data": { "goods_receipt_id": receipt.id } } ] },
        }),
    )
    .await;
    assert!(invoice.record_no.as_deref().unwrap().starts_with("PI20"));
    assert_eq!(invoice.title, "CD12345678 永豐材料");
    assert_eq!(invoice.data["status"], "draft");
    assert_eq!(invoice.data["doc_type"], "triplicate");
    assert_eq!(invoice.data["tax_type"], "taxable");
    assert_eq!(invoice.data["deductible"], true);
    assert_eq!(invoice.data["tax_period"], "202611");
    assert_eq!(invoice.data["subtotal"], "200.0000");
    let links = lines(&invoice, "purchase_invoice_receipt");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].data["goods_receipt_id"], json!(receipt.id));

    let allowance = create(
        &pool,
        &token,
        "purchase_allowance",
        json!({
            "data": {
                "purchase_invoice_id": invoice.id, "allowance_date": "2026-11-20",
                "subtotal": "20", "tax_amount": "1",
                "supplier_id": Id::now_v7(), "tax_type": "exempt",
            },
        }),
    )
    .await;
    assert!(allowance.record_no.as_deref().unwrap().starts_with("PA20"));
    assert!(allowance.title.contains("永豐材料"), "{}", allowance.title);
    assert_eq!(allowance.data["supplier_id"], json!(supplier));
    assert_eq!(allowance.data["tax_type"], "taxable");
    assert_eq!(allowance.data["tax_period"], "202611");

    // 不可更換原憑證
    let other = create(
        &pool,
        &token,
        "purchase_invoice",
        json!({ "data": { "invoice_no": "1234567890", "invoice_date": "2026-11-12",
                          "supplier_id": supplier, "doc_type": "customs",
                          "subtotal": "0", "tax_amount": "0" } }),
    )
    .await;
    let uri = format!("{}/{}", records_uri("purchase_allowance"), allowance.id);
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &uri,
        &token,
        Some(json!({ "version": allowance.version, "data": { "purchase_invoice_id": other.id } })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([{ "field_key": "purchase_invoice_id", "message": "建立後不可更換憑證" }])
    );
}

/// 必填欄位：發票號碼、日期、客戶；進項憑證的金額。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn required_fields(pool: PgPool) {
    let (_, token) = editor(&pool).await;
    let (customer, supplier, _) = master_data(&pool, &token).await;
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("sales_invoice"),
        &token,
        Some(json!({ "data": { "customer_id": customer, "tax_type": "taxable" } })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let keys: Vec<&str> = body["details"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["field_key"].as_str().unwrap())
        .collect();
    assert!(
        keys.contains(&"invoice_no") && keys.contains(&"invoice_date"),
        "{body}"
    );

    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("purchase_invoice"),
        &token,
        Some(
            json!({ "data": { "supplier_id": supplier, "invoice_no": "CD12345678",
                               "invoice_date": "2026-11-12" } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let keys: Vec<&str> = body["details"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["field_key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["subtotal", "tax_amount"], "{body}");
}

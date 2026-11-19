//! 銷貨折讓的整合測試（#185，docs/architecture.md §8.11「銷貨折讓」）：原發票必須已開立、折讓日期、
//! 累計上限（含並行開立同一張發票的折讓）、作廢後額度恢復、關聯銷貨退回與退回單的作廢限制。
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

async fn user(pool: &PgPool, name: &str, role: &str) -> String {
    let (_, token) = create_user_with_session(pool, &TestUser::new(name, role))
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

async fn create(pool: &PgPool, token: &str, entity: &str, body: Value) -> Record {
    let (status, body) = call(pool, Method::POST, &records_uri(entity), token, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn action(
    pool: &PgPool,
    token: &str,
    entity: &str,
    id: Id,
    name: &str,
    version: i32,
) -> (StatusCode, Value) {
    call(
        pool,
        Method::POST,
        &format!("{}/{name}", record_uri(entity, id)),
        token,
        Some(json!({ "version": version })),
    )
    .await
}

async fn void(
    pool: &PgPool,
    token: &str,
    entity: &str,
    id: Id,
    version: i32,
    extra: Value,
) -> (StatusCode, Value) {
    let mut body = json!({ "version": version });
    body.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    call(
        pool,
        Method::POST,
        &format!("{}/void", record_uri(entity, id)),
        token,
        Some(body),
    )
    .await
}

struct Master {
    customer: Id,
    other_customer: Id,
    service: Id,
}

/// 客戶（有發票抬頭）、另一個客戶、服務類商品（出貨不需要庫存）。
async fn master_data(pool: &PgPool, token: &str) -> Master {
    let customer = create(
        pool,
        token,
        "customer",
        json!({ "data": { "code": "C001", "name": "大華", "tax_id": "12345678",
                          "invoice_title": "大華股份有限公司", "address": "台北市",
                          "invoice_address": "台北市信義區" } }),
    )
    .await
    .id;
    let other_customer = create(
        pool,
        token,
        "customer",
        json!({ "data": { "code": "C002", "name": "小明商行", "address": "新竹市" } }),
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
    let service = create(
        pool,
        token,
        "product",
        json!({ "data": { "code": "FEE", "name": "安裝費", "unit_id": unit,
                          "product_type": "service" } }),
    )
    .await
    .id;
    Master {
        customer,
        other_customer,
        service,
    }
}

/// 已過帳的出貨單（服務類商品）。
async fn posted_shipment(pool: &PgPool, token: &str, customer: Id, service: Id) -> Record {
    let sh = create(
        pool,
        token,
        "shipment",
        json!({
            "data": { "customer_id": customer, "ship_date": "2026-11-10", "tax_type": "taxable" },
            "lines": { "shipment_line": [
                { "data": { "product_id": service, "qty": "1", "unit_price": "1000" } },
            ] },
        }),
    )
    .await;
    let (status, body) = action(pool, token, "shipment", sh.id, "post", sh.version).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

fn invoice_body(customer: Id, no: &str, shipments: &[Id]) -> Value {
    json!({
        "data": { "invoice_no": no, "invoice_date": "2026-11-19", "customer_id": customer,
                  "tax_type": "taxable" },
        "lines": {
            "sales_invoice_line": [ { "data": { "description": "安裝費", "qty": "1", "unit_price": "1000" } } ],
            "sales_invoice_shipment": shipments.iter().map(|id| json!({ "data": { "shipment_id": id } })).collect::<Vec<_>>(),
        },
    })
}

async fn invoice(pool: &PgPool, token: &str, customer: Id, no: &str, shipments: &[Id]) -> Record {
    create(
        pool,
        token,
        "sales_invoice",
        invoice_body(customer, no, shipments),
    )
    .await
}

async fn post_ok(pool: &PgPool, token: &str, entity: &str, record: &Record) -> Record {
    let (status, body) = action(pool, token, entity, record.id, "post", record.version).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

/// 已開立的三聯式發票：未稅 2000、稅額 100、總計 2100，關聯一張已過帳的出貨單。回傳 `(發票, 出貨單)`。
async fn posted_invoice(pool: &PgPool, token: &str, m: &Master) -> (Record, Record) {
    let sh = posted_shipment(pool, token, m.customer, m.service).await;
    let inv = create(
        pool,
        token,
        "sales_invoice",
        json!({
            "data": { "invoice_no": "AB12345678", "invoice_date": "2026-11-19",
                      "customer_id": m.customer, "tax_type": "taxable" },
            "lines": {
                "sales_invoice_line": [ { "data": { "description": "安裝費", "qty": "2", "unit_price": "1000" } } ],
                "sales_invoice_shipment": [ { "data": { "shipment_id": sh.id } } ],
            },
        }),
    )
    .await;
    assert_eq!(inv.data["total"], "2100.0000");
    (post_ok(pool, token, "sales_invoice", &inv).await, sh)
}

fn allowance_body(invoice: Id, price: &str, sales_return: Option<Id>) -> Value {
    json!({
        "data": { "sales_invoice_id": invoice, "allowance_date": "2026-11-25",
                  "sales_return_id": sales_return, "reason": "降價" },
        "lines": { "sales_allowance_line": [
            { "data": { "description": "安裝費折讓", "qty": "1", "unit_price": price } },
        ] },
    })
}

async fn allowance(pool: &PgPool, token: &str, invoice: Id, price: &str) -> Record {
    create(
        pool,
        token,
        "sales_allowance",
        allowance_body(invoice, price, None),
    )
    .await
}

/// 原發票必須已開立；折讓日期不可早於發票日期；金額一律未稅。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn invoice_must_be_posted(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    let draft = invoice(&pool, &token, m.customer, "AB12345679", &[]).await;
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("sales_allowance"),
        &token,
        Some(json!({
            "data": { "sales_invoice_id": draft.id, "allowance_date": "2026-11-18" },
            "lines": { "sales_allowance_line": [
                { "data": { "description": "x", "qty": "1", "unit_price": "1" } },
            ] },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([
            { "field_key": "sales_invoice_id", "message": "發票尚未開立" },
            { "field_key": "allowance_date", "message": "折讓日期不可早於發票日期" },
        ])
    );

    let (inv, _) = posted_invoice(&pool, &token, &m).await;
    let a = allowance(&pool, &token, inv.id, "1000").await;
    assert_eq!(a.data["subtotal"], "1000.0000");
    assert_eq!(a.data["tax_amount"], "50.0000");
    assert_eq!(a.data["total"], "1050.0000");
    assert_eq!(a.data["customer_id"], json!(m.customer));
    let a = post_ok(&pool, &token, "sales_allowance", &a).await;
    assert_eq!(a.data["status"], "posted");

    // 發票作廢後不能再開立折讓（先作廢折讓才能作廢發票）
    let (status, _) = void(
        &pool,
        &token,
        "sales_allowance",
        a.id,
        a.version,
        json!({ "void_reason": "開錯", "void_date": "2026-11-30" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = void(
        &pool,
        &token,
        "sales_invoice",
        inv.id,
        inv.version,
        json!({ "void_reason": "重開" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("sales_allowance"),
        &token,
        Some(allowance_body(inv.id, "1", None)),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([{ "field_key": "sales_invoice_id", "message": "發票已作廢" }])
    );
}

/// 累計上限：超過時 422 並列出可折讓金額；作廢折讓後額度恢復；開立時以最新的已折讓金額檢查。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn cumulative_cap(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    let (inv, _) = posted_invoice(&pool, &token, &m).await;

    // 超過發票總計
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("sales_allowance"),
        &token,
        Some(allowance_body(inv.id, "2001", None)),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([{ "field_key": "total",
                 "message": "超過可折讓金額 2,100（發票總計 2,100，已折讓 0）" }])
    );

    let a = allowance(&pool, &token, inv.id, "1000").await;
    // 草稿不佔額度：b 可以存檔
    let b = allowance(&pool, &token, inv.id, "1001").await;
    assert_eq!(b.data["total"], "1051.0000");
    let a = post_ok(&pool, &token, "sales_allowance", &a).await;

    // 開立 b 時已折讓 1050，剩 1050
    let (status, body) = action(&pool, &token, "sales_allowance", b.id, "post", b.version).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([{ "field_key": "total",
                 "message": "超過可折讓金額 1,050（發票總計 2,100，已折讓 1,050）" }])
    );
    // 新存檔的也一樣
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("sales_allowance"),
        &token,
        Some(allowance_body(inv.id, "1001", None)),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["details"]["fields"][0]["field_key"], "total");

    // 作廢 a 後額度恢復
    let (status, body) = void(
        &pool,
        &token,
        "sales_allowance",
        a.id,
        a.version,
        json!({ "void_reason": "金額錯誤", "void_date": "2026-11-26" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["void_reason"], "金額錯誤");
    let b = post_ok(&pool, &token, "sales_allowance", &b).await;
    assert_eq!(b.data["status"], "posted");

    // 作廢：原因必填、日期不可早於折讓日期
    let (status, body) = void(
        &pool,
        &token,
        "sales_allowance",
        b.id,
        b.version,
        json!({ "void_date": "2026-11-24" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([
            { "field_key": "void_reason", "message": "請輸入作廢原因" },
            { "field_key": "void_date", "message": "作廢日期不可早於折讓日期" },
        ])
    );
}

/// 並行開立同一張發票的兩張折讓：合計超過發票總計時只有一張成功。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn concurrent_allowances_respect_cap(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    let (inv, _) = posted_invoice(&pool, &token, &m).await;
    let a = allowance(&pool, &token, inv.id, "1500").await;
    let b = allowance(&pool, &token, inv.id, "1500").await;
    let (ra, rb) = tokio::join!(
        action(&pool, &token, "sales_allowance", a.id, "post", 1),
        action(&pool, &token, "sales_allowance", b.id, "post", 1),
    );
    let mut statuses = [ra.0, rb.0];
    statuses.sort();
    assert_eq!(
        statuses,
        [StatusCode::OK, StatusCode::UNPROCESSABLE_ENTITY],
        "{} {}",
        ra.1,
        rb.1
    );
    let failed = if ra.0 == StatusCode::OK { &rb.1 } else { &ra.1 };
    assert_eq!(
        failed["details"]["fields"][0]["message"],
        "超過可折讓金額 525（發票總計 2,100，已折讓 1,575）"
    );
}

/// 關聯銷貨退回：已過帳、同客戶、只在一張有效折讓中；被關聯的退回單不可作廢。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn sales_return_link(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    let (inv, sh) = posted_invoice(&pool, &token, &m).await;
    let sh_line = sh.lines.as_ref().unwrap()["shipment_line"][0].id;
    let sr = create(
        &pool,
        &token,
        "sales_return",
        json!({
            "data": { "shipment_id": sh.id, "return_date": "2026-11-20" },
            "lines": { "sales_return_line": [ { "data": { "shipment_line_id": sh_line, "qty": "1" } } ] },
        }),
    )
    .await;

    // 草稿的退回單
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("sales_allowance"),
        &token,
        Some(allowance_body(inv.id, "100", Some(sr.id))),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([{ "field_key": "sales_return_id", "message": "銷貨退回尚未過帳" }])
    );

    let sr = post_ok(&pool, &token, "sales_return", &sr).await;
    let a = create(
        &pool,
        &token,
        "sales_allowance",
        allowance_body(inv.id, "100", Some(sr.id)),
    )
    .await;
    let b = create(
        &pool,
        &token,
        "sales_allowance",
        allowance_body(inv.id, "100", Some(sr.id)),
    )
    .await;
    let a = post_ok(&pool, &token, "sales_allowance", &a).await;
    let message = format!(
        "銷貨退回 {} 已有折讓單 {}",
        sr.record_no.clone().unwrap(),
        a.record_no.clone().unwrap()
    );
    let (status, body) = action(&pool, &token, "sales_allowance", b.id, "post", b.version).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([{ "field_key": "sales_return_id", "message": message }])
    );

    // 退回單被有效折讓關聯時不可作廢
    let (status, body) = action(&pool, &token, "sales_return", sr.id, "void", sr.version).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["message"],
        format!(
            "此銷貨退回已有折讓單 {}，請先作廢折讓單",
            a.record_no.clone().unwrap()
        )
    );
    assert_eq!(
        body["details"]["documents"][0]["entity_key"],
        "sales_allowance"
    );

    // 作廢折讓後，退回單可以關聯到另一張折讓，也可以作廢
    let (status, _) = void(
        &pool,
        &token,
        "sales_allowance",
        a.id,
        a.version,
        json!({ "void_reason": "x", "void_date": "2026-11-30" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let b = post_ok(&pool, &token, "sales_allowance", &b).await;
    let (status, _) = void(
        &pool,
        &token,
        "sales_allowance",
        b.id,
        b.version,
        json!({ "void_reason": "x", "void_date": "2026-11-30" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = action(&pool, &token, "sales_return", sr.id, "void", sr.version).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // 其他客戶的退回單
    let other_sh = posted_shipment(&pool, &token, m.other_customer, m.service).await;
    let other_line = other_sh.lines.as_ref().unwrap()["shipment_line"][0].id;
    let other_sr = create(
        &pool,
        &token,
        "sales_return",
        json!({
            "data": { "shipment_id": other_sh.id, "return_date": "2026-11-20" },
            "lines": { "sales_return_line": [ { "data": { "shipment_line_id": other_line, "qty": "1" } } ] },
        }),
    )
    .await;
    let other_sr = post_ok(&pool, &token, "sales_return", &other_sr).await;
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("sales_allowance"),
        &token,
        Some(allowance_body(inv.id, "100", Some(other_sr.id))),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([{ "field_key": "sales_return_id", "message": "客戶與發票不同" }])
    );
}

/// 開立時至少一列明細。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn posting_requires_lines(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    let (inv, _) = posted_invoice(&pool, &token, &m).await;
    let a = create(
        &pool,
        &token,
        "sales_allowance",
        json!({ "data": { "sales_invoice_id": inv.id, "allowance_date": "2026-11-25" } }),
    )
    .await;
    assert_eq!(a.data["total"], "0.0000");
    let (status, body) = action(&pool, &token, "sales_allowance", a.id, "post", a.version).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["lines"],
        json!([{ "detail_key": "sales_allowance_line", "index": null, "field_key": null,
                 "message": "至少要有一筆明細" }])
    );
}

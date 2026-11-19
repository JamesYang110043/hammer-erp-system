//! 進項憑證與進貨折讓的整合測試（#186，docs/architecture.md §8.11）：號碼格式與「同一廠商 + 號碼」的唯一，
//! 金額與稅額檢查，賣方統編帶入，關聯進貨單（發票類與海關繳納證分開、進貨單的作廢限制），進貨折讓（海關
//! 繳納證不可折讓、累計上限、關聯進貨退回與退回單的作廢限制），廠商的刪除限制。
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

struct Master {
    supplier: Id,
    other_supplier: Id,
    service: Id,
}

/// 兩個廠商（第一個有統編）與服務類商品（進貨不建立批號）。
async fn master_data(pool: &PgPool, token: &str) -> Master {
    let supplier = create(
        pool,
        token,
        "supplier",
        json!({ "data": { "code": "S001", "name": "永豐材料", "tax_id": "87654321",
                          "default_currency_code": "TWD" } }),
    )
    .await
    .id;
    let other_supplier = create(
        pool,
        token,
        "supplier",
        json!({ "data": { "code": "S002", "name": "Acme Inc.", "default_currency_code": "TWD" } }),
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
        json!({ "data": { "code": "FEE", "name": "加工費", "unit_id": unit,
                          "product_type": "service" } }),
    )
    .await
    .id;
    Master {
        supplier,
        other_supplier,
        service,
    }
}

async fn post_ok(pool: &PgPool, token: &str, entity: &str, record: &Record) -> Record {
    let (status, body) = action(pool, token, entity, record.id, "post", record.version).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn void(
    pool: &PgPool,
    token: &str,
    entity: &str,
    record: &Record,
    reason: &str,
) -> (StatusCode, Value) {
    call(
        pool,
        Method::POST,
        &format!("{}/void", record_uri(entity, record.id)),
        token,
        Some(
            json!({ "version": record.version, "void_reason": reason, "void_date": "2026-11-30" }),
        ),
    )
    .await
}

/// 已過帳的進貨單（台幣、服務類商品）。
async fn posted_receipt(pool: &PgPool, token: &str, supplier: Id, service: Id) -> Record {
    let gr = create(
        pool,
        token,
        "goods_receipt",
        json!({
            "data": { "supplier_id": supplier, "receipt_date": "2026-11-10", "currency_code": "TWD",
                      "tax_type": "taxable" },
            "lines": { "goods_receipt_line": [
                { "data": { "product_id": service, "qty": "2", "unit_price": "100" } },
            ] },
        }),
    )
    .await;
    post_ok(pool, token, "goods_receipt", &gr).await
}

fn invoice_body(supplier: Id, doc_type: &str, no: &str, receipts: &[Id]) -> Value {
    json!({
        "data": { "doc_type": doc_type, "invoice_no": no, "invoice_date": "2026-11-19",
                  "supplier_id": supplier, "subtotal": "200", "tax_amount": "10" },
        "lines": { "purchase_invoice_receipt": receipts.iter()
            .map(|id| json!({ "data": { "goods_receipt_id": id } })).collect::<Vec<_>>() },
    })
}

async fn invoice(pool: &PgPool, token: &str, supplier: Id, no: &str, receipts: &[Id]) -> Record {
    create(
        pool,
        token,
        "purchase_invoice",
        invoice_body(supplier, "triplicate", no, receipts),
    )
    .await
}

async fn post_error(pool: &PgPool, token: &str, entity: &str, body: Value) -> Value {
    let (status, body) = call(pool, Method::POST, &records_uri(entity), token, Some(body)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    body
}

/// 金額、稅額檢查、號碼格式、賣方統編帶入。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn amounts_and_formats(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    let inv = invoice(&pool, &token, m.supplier, " cd12345678", &[]).await;
    assert_eq!(inv.data["invoice_no"], "CD12345678");
    assert_eq!(inv.data["total"], "210.0000");
    assert_eq!(inv.data["seller_tax_id"], "87654321");
    assert_eq!(inv.data["deductible"], true);

    // 稅額與未稅金額 × 稅率相差超過 1 元
    let mut body = invoice_body(m.supplier, "electronic", "CD12345679", &[]);
    body["data"]["tax_amount"] = json!("12");
    let err = post_error(&pool, &token, "purchase_invoice", body).await;
    assert_eq!(
        err["details"]["fields"],
        json!([{ "field_key": "tax_amount",
                 "message": "稅額與未稅金額 × 稅率相差超過 1 元（應為 10）" }])
    );

    // 海關繳納證：英數字號碼、不檢查稅額
    let mut body = invoice_body(m.other_supplier, "customs", "aa12345678901", &[]);
    body["data"]["tax_amount"] = json!("13");
    let (status, customs) = call(
        &pool,
        Method::POST,
        &records_uri("purchase_invoice"),
        &token,
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{customs}");
    assert_eq!(customs["data"]["invoice_no"], "AA12345678901");
    assert_eq!(customs["data"]["total"], "213.0000");
    assert_eq!(customs["data"]["seller_tax_id"], Value::Null);
    let err = post_error(
        &pool,
        &token,
        "purchase_invoice",
        invoice_body(m.other_supplier, "customs", "AA-1", &[]),
    )
    .await;
    assert_eq!(
        err["details"]["fields"],
        json!([{ "field_key": "invoice_no", "message": "繳納證號碼只能是英文與數字，最長 20 碼" }])
    );
    let err = post_error(
        &pool,
        &token,
        "purchase_invoice",
        invoice_body(m.supplier, "triplicate", "AA12345678901", &[]),
    )
    .await;
    assert_eq!(
        err["details"]["fields"],
        json!([{ "field_key": "invoice_no", "message": "發票號碼格式應為 2 碼英文加 8 碼數字" }])
    );

    // 未稅金額為 0 → 登錄時 422
    let mut body = invoice_body(m.supplier, "triplicate", "CD12345670", &[]);
    body["data"]["subtotal"] = json!("0");
    body["data"]["tax_amount"] = json!("0");
    let zero = create(&pool, &token, "purchase_invoice", body).await;
    let (status, body) = action(&pool, &token, "purchase_invoice", zero.id, "post", 1).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([{ "field_key": "subtotal", "message": "未稅金額必須大於 0" }])
    );
}

/// 號碼：同一廠商 + 號碼在已登錄中唯一；作廢後可以重新登錄；不同廠商不限制。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn number_unique_per_supplier(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    let a = invoice(&pool, &token, m.supplier, "CD12345678", &[]).await;
    let b = invoice(&pool, &token, m.supplier, "CD12345678", &[]).await;
    let c = invoice(&pool, &token, m.other_supplier, "CD12345678", &[]).await;
    let a = post_ok(&pool, &token, "purchase_invoice", &a).await;
    post_ok(&pool, &token, "purchase_invoice", &c).await;
    let (status, body) = action(&pool, &token, "purchase_invoice", b.id, "post", b.version).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([{ "field_key": "invoice_no",
                 "message": format!("此廠商的憑證號碼 CD12345678 已登錄（{}）", a.record_no.clone().unwrap()) }])
    );
    let (status, body) = void(&pool, &token, "purchase_invoice", &a, "登錄錯誤").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["void_reason"], "登錄錯誤");
    post_ok(&pool, &token, "purchase_invoice", &b).await;
}

/// 關聯進貨單：發票類只能一張有效、海關繳納證另外計算；被關聯的進貨單不可作廢，作廢憑證後可以。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn receipt_links(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    let gr = posted_receipt(&pool, &token, m.supplier, m.service).await;
    let other = posted_receipt(&pool, &token, m.other_supplier, m.service).await;

    let err = post_error(
        &pool,
        &token,
        "purchase_invoice",
        invoice_body(
            m.supplier,
            "triplicate",
            "CD12345678",
            &[other.id, gr.id, gr.id],
        ),
    )
    .await;
    let line = |index: usize, message: &str| {
        json!({ "detail_key": "purchase_invoice_receipt", "index": index,
                "field_key": "goods_receipt_id", "message": message })
    };
    assert_eq!(
        err["details"]["lines"],
        json!([line(0, "廠商與憑證不同"), line(2, "進貨單重複")])
    );

    let a = invoice(&pool, &token, m.supplier, "CD12345678", &[gr.id]).await;
    let b = invoice(&pool, &token, m.supplier, "CD12345679", &[gr.id]).await;
    let a = post_ok(&pool, &token, "purchase_invoice", &a).await;
    let (status, body) = action(&pool, &token, "purchase_invoice", b.id, "post", b.version).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["lines"],
        json!([line(
            0,
            &format!(
                "進貨單 {} 已有憑證 CD12345678",
                gr.record_no.clone().unwrap()
            )
        )])
    );

    // 海關繳納證與發票類各自計算
    let customs = create(
        &pool,
        &token,
        "purchase_invoice",
        invoice_body(m.supplier, "customs", "AA123", &[gr.id]),
    )
    .await;
    let customs = post_ok(&pool, &token, "purchase_invoice", &customs).await;

    // 進貨單被有效憑證關聯時不可作廢
    let (status, body) = action(&pool, &token, "goods_receipt", gr.id, "void", gr.version).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["message"],
        "此進貨單已有進項憑證 CD12345678，請先作廢憑證"
    );
    assert_eq!(body["details"]["documents"].as_array().unwrap().len(), 2);

    for record in [&a, &customs] {
        let (status, body) = void(&pool, &token, "purchase_invoice", record, "x").await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let (status, body) = action(&pool, &token, "goods_receipt", gr.id, "void", gr.version).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

fn allowance_body(invoice: Id, subtotal: &str, tax: &str, purchase_return: Option<Id>) -> Value {
    json!({ "data": { "purchase_invoice_id": invoice, "allowance_date": "2026-11-25",
                      "subtotal": subtotal, "tax_amount": tax,
                      "purchase_return_id": purchase_return } })
}

/// 進貨折讓：海關繳納證不可折讓；未稅金額必須大於 0；累計上限；作廢後額度恢復；有效折讓時憑證不可作廢。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn purchase_allowances(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    let customs = create(
        &pool,
        &token,
        "purchase_invoice",
        invoice_body(m.supplier, "customs", "AA123", &[]),
    )
    .await;
    let customs = post_ok(&pool, &token, "purchase_invoice", &customs).await;
    let err = post_error(
        &pool,
        &token,
        "purchase_allowance",
        allowance_body(customs.id, "10", "1", None),
    )
    .await;
    assert_eq!(
        err["details"]["fields"],
        json!([{ "field_key": "purchase_invoice_id", "message": "海關繳納證不可折讓" }])
    );

    let inv = invoice(&pool, &token, m.supplier, "CD12345678", &[]).await;
    let inv = post_ok(&pool, &token, "purchase_invoice", &inv).await;
    let err = post_error(
        &pool,
        &token,
        "purchase_allowance",
        allowance_body(inv.id, "0", "0", None),
    )
    .await;
    assert_eq!(
        err["details"]["fields"],
        json!([{ "field_key": "subtotal", "message": "未稅金額必須大於 0" }])
    );

    let a = create(
        &pool,
        &token,
        "purchase_allowance",
        allowance_body(inv.id, "100", "5", None),
    )
    .await;
    assert_eq!(a.data["total"], "105.0000");
    assert_eq!(a.data["supplier_id"], json!(m.supplier));
    let a = post_ok(&pool, &token, "purchase_allowance", &a).await;
    let b = create(
        &pool,
        &token,
        "purchase_allowance",
        allowance_body(inv.id, "100", "5", None),
    )
    .await;
    let c = create(
        &pool,
        &token,
        "purchase_allowance",
        allowance_body(inv.id, "1", "0", None),
    )
    .await;
    post_ok(&pool, &token, "purchase_allowance", &b).await;
    let (status, body) = action(&pool, &token, "purchase_allowance", c.id, "post", c.version).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([{ "field_key": "total",
                 "message": "超過可折讓金額 0（憑證總計 210，已折讓 210）" }])
    );

    // 有效折讓時憑證不可作廢
    let (status, body) = void(&pool, &token, "purchase_invoice", &inv, "x").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .starts_with("此憑證已有進貨折讓 "),
        "{body}"
    );

    // 作廢 a 後額度恢復
    let (status, body) = void(&pool, &token, "purchase_allowance", &a, "x").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    post_ok(&pool, &token, "purchase_allowance", &c).await;
}

/// 關聯進貨退回：已過帳、同廠商、只在一張有效折讓中；被關聯的退回單不可作廢。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn purchase_return_link(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    let gr = posted_receipt(&pool, &token, m.supplier, m.service).await;
    let gr_line = gr.lines.as_ref().unwrap()["goods_receipt_line"][0].id;
    let pr = create(
        &pool,
        &token,
        "purchase_return",
        json!({
            "data": { "goods_receipt_id": gr.id, "return_date": "2026-11-20" },
            "lines": { "purchase_return_line": [ { "data": { "goods_receipt_line_id": gr_line, "qty": "1" } } ] },
        }),
    )
    .await;
    let inv = invoice(&pool, &token, m.supplier, "CD12345678", &[gr.id]).await;
    let inv = post_ok(&pool, &token, "purchase_invoice", &inv).await;

    let err = post_error(
        &pool,
        &token,
        "purchase_allowance",
        allowance_body(inv.id, "100", "5", Some(pr.id)),
    )
    .await;
    assert_eq!(
        err["details"]["fields"],
        json!([{ "field_key": "purchase_return_id", "message": "進貨退回尚未過帳" }])
    );
    let pr = post_ok(&pool, &token, "purchase_return", &pr).await;
    let a = create(
        &pool,
        &token,
        "purchase_allowance",
        allowance_body(inv.id, "100", "5", Some(pr.id)),
    )
    .await;
    let a = post_ok(&pool, &token, "purchase_allowance", &a).await;
    let err = post_error(
        &pool,
        &token,
        "purchase_allowance",
        allowance_body(inv.id, "10", "1", Some(pr.id)),
    )
    .await;
    assert_eq!(
        err["details"]["fields"],
        json!([{ "field_key": "purchase_return_id",
                 "message": format!("進貨退回 {} 已有折讓單 {}",
                                    pr.record_no.clone().unwrap(), a.record_no.clone().unwrap()) }])
    );

    let (status, body) = action(&pool, &token, "purchase_return", pr.id, "void", pr.version).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["message"],
        format!(
            "此進貨退回已有折讓單 {}，請先作廢折讓單",
            a.record_no.clone().unwrap()
        )
    );
    let (status, _) = void(&pool, &token, "purchase_allowance", &a, "x").await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = action(&pool, &token, "purchase_return", pr.id, "void", pr.version).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// 廠商被進項憑證引用時不可刪除。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn supplier_with_invoice_cannot_be_deleted(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    invoice(&pool, &token, m.other_supplier, "CD12345678", &[]).await;
    let (status, body) = call(
        &pool,
        Method::DELETE,
        &format!("{}?version=1", record_uri("supplier", m.other_supplier)),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["message"],
        "此廠商已有 1 張進項憑證或折讓單，無法刪除，請改為停用"
    );
    assert_eq!(
        body["details"],
        json!({ "fields": [], "invoice_document_count": 1 })
    );
}

//! 發票模組查詢 API 的整合測試（#187，docs/architecture.md §8.11「查詢 API」）：尚未開立發票的出貨單、
//! 尚未取得憑證的進貨單（發票類與海關繳納證分開），單據的發票、憑證與折讓。
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

async fn post_ok(pool: &PgPool, token: &str, entity: &str, record: &Record) -> Record {
    let (status, body) = action(pool, token, entity, record.id, "post", record.version).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn void(pool: &PgPool, token: &str, entity: &str, record: &Record) -> Record {
    let (status, body) = call(
        pool,
        Method::POST,
        &format!("{}/void", record_uri(entity, record.id)),
        token,
        Some(json!({ "version": record.version, "void_reason": "x", "void_date": "2026-11-30" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn get(pool: &PgPool, token: &str, uri: &str) -> Value {
    let (status, body) = call(pool, Method::GET, uri, token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

fn record_nos(page: &Value) -> Vec<String> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["record_no"].as_str().unwrap().to_owned())
        .collect()
}

struct Master {
    customer: Id,
    other_customer: Id,
    supplier: Id,
    service: Id,
}

async fn master_data(pool: &PgPool, token: &str) -> Master {
    let mut ids = Vec::new();
    for (entity, data) in [
        (
            "customer",
            json!({ "code": "C001", "name": "大華", "tax_id": "12345678" }),
        ),
        ("customer", json!({ "code": "C002", "name": "小明" })),
        (
            "supplier",
            json!({ "code": "S001", "name": "永豐", "default_currency_code": "TWD" }),
        ),
        ("unit", json!({ "code": "pcs", "name": "個" })),
    ] {
        ids.push(
            create(pool, token, entity, json!({ "data": data }))
                .await
                .id,
        );
    }
    let service = create(
        pool,
        token,
        "product",
        json!({ "data": { "code": "FEE", "name": "服務費", "unit_id": ids[3],
                          "product_type": "service" } }),
    )
    .await
    .id;
    Master {
        customer: ids[0],
        other_customer: ids[1],
        supplier: ids[2],
        service,
    }
}

async fn shipment(pool: &PgPool, token: &str, customer: Id, service: Id, date: &str) -> Record {
    create(
        pool,
        token,
        "shipment",
        json!({
            "data": { "customer_id": customer, "ship_date": date, "tax_type": "taxable" },
            "lines": { "shipment_line": [
                { "data": { "product_id": service, "qty": "2", "unit_price": "500" } },
            ] },
        }),
    )
    .await
}

async fn receipt(pool: &PgPool, token: &str, supplier: Id, service: Id) -> Record {
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

/// 尚未開立發票的出貨單：草稿、已作廢、已開立發票的不出現；發票作廢後重新出現；篩選與退回淨額。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn uninvoiced_shipments(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let viewer = user(&pool, "viewer", role::VIEWER).await;
    let m = master_data(&pool, &token).await;
    let a = shipment(&pool, &token, m.customer, m.service, "2026-11-10").await;
    let a = post_ok(&pool, &token, "shipment", &a).await;
    let b = shipment(&pool, &token, m.customer, m.service, "2026-11-05").await;
    let b = post_ok(&pool, &token, "shipment", &b).await;
    let _draft = shipment(&pool, &token, m.customer, m.service, "2026-11-01").await;
    let voided = shipment(&pool, &token, m.customer, m.service, "2026-11-02").await;
    let voided = post_ok(&pool, &token, "shipment", &voided).await;
    let (status, _) = action(&pool, &token, "shipment", voided.id, "void", voided.version).await;
    assert_eq!(status, StatusCode::OK);
    let other = shipment(&pool, &token, m.other_customer, m.service, "2026-11-03").await;
    let other = post_ok(&pool, &token, "shipment", &other).await;
    let no = |r: &Record| r.record_no.clone().unwrap();

    // 唯讀者也可以查詢；依出貨日期排序
    let uri = "/api/v1/invoicing/uninvoiced-shipments";
    let page = get(&pool, &viewer, uri).await;
    assert_eq!(record_nos(&page), [no(&other), no(&b), no(&a)]);
    assert_eq!(page["total"], 3);

    // 客戶、日期篩選
    let page = get(
        &pool,
        &token,
        &format!(
            "{uri}?customer_id={}&date_from=2026-11-06&date_to=2026-11-10",
            m.customer
        ),
    )
    .await;
    assert_eq!(record_nos(&page), [no(&a)]);
    let item = &page["items"][0];
    assert_eq!(item["customer_name"], "大華");
    assert_eq!(item["ship_date"], "2026-11-10");
    assert_eq!(item["total"], "1050.0000");
    assert_eq!(item["returned_total"], "0.0000");
    assert_eq!(item["net_total"], "1050.0000");

    // 銷貨退回後淨額減少
    let line = a.lines.as_ref().unwrap()["shipment_line"][0].id;
    let sr = create(
        &pool,
        &token,
        "sales_return",
        json!({
            "data": { "shipment_id": a.id, "return_date": "2026-11-12" },
            "lines": { "sales_return_line": [ { "data": { "shipment_line_id": line, "qty": "1" } } ] },
        }),
    )
    .await;
    post_ok(&pool, &token, "sales_return", &sr).await;
    let page = get(&pool, &token, &format!("{uri}?date_from=2026-11-10")).await;
    assert_eq!(page["items"][0]["returned_total"], "525.0000");
    assert_eq!(page["items"][0]["net_total"], "525.0000");

    // 開立發票後不出現；作廢發票後重新出現
    let inv = create(
        &pool,
        &token,
        "sales_invoice",
        json!({
            "data": { "invoice_no": "AB12345678", "invoice_date": "2026-11-19",
                      "customer_id": m.customer, "tax_type": "taxable" },
            "lines": {
                "sales_invoice_line": [ { "data": { "description": "服務費", "qty": "1", "unit_price": "1000" } } ],
                "sales_invoice_shipment": [ { "data": { "shipment_id": a.id } } ],
            },
        }),
    )
    .await;
    // 草稿的發票不影響
    assert_eq!(get(&pool, &token, uri).await["total"], 3);
    let inv = post_ok(&pool, &token, "sales_invoice", &inv).await;
    assert_eq!(
        record_nos(&get(&pool, &token, uri).await),
        [no(&other), no(&b)]
    );
    void(&pool, &token, "sales_invoice", &inv).await;
    assert_eq!(get(&pool, &token, uri).await["total"], 3);

    // 參數錯誤
    for (query, field) in [
        ("date_from=2026/11/01", "date_from"),
        ("date_from=2026-11-10&date_to=2026-11-01", "date_from"),
        ("customer_id=abc", "customer_id"),
        ("page=0", "page"),
    ] {
        let (status, body) =
            call(&pool, Method::GET, &format!("{uri}?{query}"), &token, None).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{query} {body}");
        assert_eq!(body["details"]["field"], field, "{query}");
    }
}

/// 尚未取得憑證的進貨單：發票類與海關繳納證分開計算。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn uninvoiced_receipts(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    let gr = receipt(&pool, &token, m.supplier, m.service).await;
    let uri = "/api/v1/invoicing/uninvoiced-receipts";
    let page = get(&pool, &token, uri).await;
    assert_eq!(page["total"], 1);
    let item = &page["items"][0];
    assert_eq!(item["supplier_name"], "永豐");
    assert_eq!(item["currency_code"], "TWD");
    assert_eq!(item["exchange_rate"], "1.000000");
    assert_eq!(item["total"], "210.0000");
    assert_eq!(item["total_twd"], "210.00");
    assert_eq!(item["net_total"], "210.0000");

    let inv = create(
        &pool,
        &token,
        "purchase_invoice",
        json!({
            "data": { "invoice_no": "CD12345678", "invoice_date": "2026-11-19",
                      "supplier_id": m.supplier, "subtotal": "200", "tax_amount": "10" },
            "lines": { "purchase_invoice_receipt": [ { "data": { "goods_receipt_id": gr.id } } ] },
        }),
    )
    .await;
    post_ok(&pool, &token, "purchase_invoice", &inv).await;
    assert_eq!(get(&pool, &token, uri).await["total"], 0);
    // 海關繳納證另外計算：仍未取得
    let customs = get(&pool, &token, &format!("{uri}?doc_class=customs")).await;
    assert_eq!(customs["total"], 1);
    assert_eq!(
        get(
            &pool,
            &token,
            &format!("{uri}?supplier_id={}&doc_class=invoice", m.supplier)
        )
        .await["total"],
        0
    );

    let (status, body) = call(
        &pool,
        Method::GET,
        &format!("{uri}?doc_class=triplicate"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["details"]["field"], "doc_class");
}

/// 單據的發票、憑證、折讓（含已作廢）；不支援的資料類別 422、不存在的單據 404。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn links(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    let sh = shipment(&pool, &token, m.customer, m.service, "2026-11-10").await;
    let sh = post_ok(&pool, &token, "shipment", &sh).await;
    let invoice = |no: &str| {
        json!({
            "data": { "invoice_no": no, "invoice_date": "2026-11-19",
                      "customer_id": m.customer, "tax_type": "taxable" },
            "lines": {
                "sales_invoice_line": [ { "data": { "description": "服務費", "qty": "1", "unit_price": "1000" } } ],
                "sales_invoice_shipment": [ { "data": { "shipment_id": sh.id } } ],
            },
        })
    };
    let first = create(&pool, &token, "sales_invoice", invoice("AB12345678")).await;
    let first = post_ok(&pool, &token, "sales_invoice", &first).await;
    void(&pool, &token, "sales_invoice", &first).await;
    let second = create(&pool, &token, "sales_invoice", invoice("AB12345679")).await;
    let second = post_ok(&pool, &token, "sales_invoice", &second).await;

    let uri =
        |key: &str, id: Id| format!("/api/v1/invoicing/links?entity_key={key}&record_id={id}");
    let links = get(&pool, &token, &uri("shipment", sh.id)).await;
    assert_eq!(
        links,
        json!([
            { "entity_key": "sales_invoice", "record_id": first.id, "record_no": first.record_no,
              "invoice_no": "AB12345678", "invoice_date": "2026-11-19", "status": "void" },
            { "entity_key": "sales_invoice", "record_id": second.id, "record_no": second.record_no,
              "invoice_no": "AB12345679", "invoice_date": "2026-11-19", "status": "posted" },
        ])
    );

    // 銷貨退回 → 折讓
    let line = sh.lines.as_ref().unwrap()["shipment_line"][0].id;
    let sr = create(
        &pool,
        &token,
        "sales_return",
        json!({
            "data": { "shipment_id": sh.id, "return_date": "2026-11-20" },
            "lines": { "sales_return_line": [ { "data": { "shipment_line_id": line, "qty": "1" } } ] },
        }),
    )
    .await;
    let sr = post_ok(&pool, &token, "sales_return", &sr).await;
    assert_eq!(
        get(&pool, &token, &uri("sales_return", sr.id)).await,
        json!([])
    );
    let allowance = create(
        &pool,
        &token,
        "sales_allowance",
        json!({
            "data": { "sales_invoice_id": second.id, "allowance_date": "2026-11-21",
                      "sales_return_id": sr.id },
            "lines": { "sales_allowance_line": [
                { "data": { "description": "退貨折讓", "qty": "1", "unit_price": "500" } },
            ] },
        }),
    )
    .await;
    let links = get(&pool, &token, &uri("sales_return", sr.id)).await;
    assert_eq!(
        links,
        json!([{ "entity_key": "sales_allowance", "record_id": allowance.id,
                 "record_no": allowance.record_no, "invoice_no": null,
                 "invoice_date": "2026-11-21", "status": "draft" }])
    );

    // 進貨單沒有憑證：空陣列
    let gr = receipt(&pool, &token, m.supplier, m.service).await;
    assert_eq!(
        get(&pool, &token, &uri("goods_receipt", gr.id)).await,
        json!([])
    );

    let (status, body) = call(
        &pool,
        Method::GET,
        &uri("customer", m.customer),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["details"]["field"], "entity_key");
    let (status, _) = call(
        &pool,
        Method::GET,
        &uri("shipment", Id::now_v7()),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = call(
        &pool,
        Method::GET,
        "/api/v1/invoicing/links?entity_key=shipment",
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["details"]["field"], "record_id");
}

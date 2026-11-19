//! 銷項發票的整合測試（#184，docs/architecture.md §8.11）：三聯式、二聯式的金額與手動稅額，發票號碼的
//! 格式與唯一，買受人帶入，關聯出貨單（已過帳、同客戶、一張出貨單只在一張有效發票中，含並行開立），
//! 開立與作廢（作廢日期與原因、有效折讓），出貨單的作廢限制，客戶的刪除限制。
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

/// 三聯式、二聯式的金額與 §8.11 的範例一致；手動稅額 ±1 元。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn amounts_match_examples(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;

    // 範例一：三聯式
    let triplicate = create(
        &pool,
        &token,
        "sales_invoice",
        json!({
            "data": { "invoice_no": " ab12345678 ", "invoice_date": "2026-11-19",
                      "customer_id": m.customer, "tax_type": "taxable" },
            "lines": { "sales_invoice_line": [
                { "data": { "description": "A", "qty": "3", "unit_price": "100" } },
                { "data": { "description": "B", "qty": "2", "unit_price": "333.3333" } },
            ] },
        }),
    )
    .await;
    assert_eq!(triplicate.data["invoice_no"], "AB12345678");
    assert_eq!(triplicate.data["subtotal"], "967.0000");
    assert_eq!(triplicate.data["tax_amount"], "48.0000");
    assert_eq!(triplicate.data["total"], "1015.0000");
    let amounts: Vec<&Value> = triplicate.lines.as_ref().unwrap()["sales_invoice_line"]
        .iter()
        .map(|l| &l.data["amount"])
        .collect();
    assert_eq!(amounts, [&json!("300.0000"), &json!("667.0000")]);
    // 買受人從客戶帶入（發票抬頭、發票地址優先）
    assert_eq!(triplicate.data["buyer_tax_id"], "12345678");
    assert_eq!(triplicate.data["buyer_name"], "大華股份有限公司");
    assert_eq!(triplicate.data["buyer_address"], "台北市信義區");

    let uri = record_uri("sales_invoice", triplicate.id);
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &uri,
        &token,
        Some(json!({ "version": triplicate.version, "data": { "tax_amount_manual": "49" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["tax_amount"], "49.0000");
    assert_eq!(body["data"]["total"], "1016.0000");
    assert_eq!(body["data"]["subtotal"], "967.0000");
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &uri,
        &token,
        Some(json!({ "version": body["version"], "data": { "tax_amount_manual": "50" } })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([{ "field_key": "tax_amount_manual",
                 "message": "稅額與系統計算相差超過 1 元（系統計算 48）" }])
    );

    // 範例二：二聯式（單價含稅）；手動稅額時總計不變
    let duplicate = create(
        &pool,
        &token,
        "sales_invoice",
        json!({
            "data": { "invoice_type": "duplicate", "invoice_no": "AB12345679",
                      "invoice_date": "2026-11-19", "customer_id": m.other_customer,
                      "tax_type": "taxable", "tax_amount_manual": "47" },
            "lines": { "sales_invoice_line": [
                { "data": { "description": "A", "qty": "4", "unit_price": "200" } },
                { "data": { "description": "B", "qty": "1", "unit_price": "200" } },
            ] },
        }),
    )
    .await;
    assert_eq!(duplicate.data["total"], "1000.0000");
    assert_eq!(duplicate.data["subtotal"], "953.0000");
    assert_eq!(duplicate.data["tax_amount"], "47.0000");
    // 客戶沒有統編、發票抬頭：抬頭用名稱，統編留空
    assert_eq!(duplicate.data["buyer_name"], "小明商行");
    assert!(
        duplicate
            .data
            .get("buyer_tax_id")
            .is_none_or(Value::is_null)
    );
    assert_eq!(duplicate.data["buyer_address"], "新竹市");

    // 零稅率不可填手動稅額
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("sales_invoice"),
        &token,
        Some(json!({
            "data": { "invoice_no": "AB12345670", "invoice_date": "2026-11-19",
                      "customer_id": m.customer, "tax_type": "zero_rated", "tax_amount_manual": "0" },
            "lines": { "sales_invoice_line": [
                { "data": { "description": "A", "qty": "1", "unit_price": "100" } },
            ] },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"][0]["message"],
        "零稅率、免稅的發票沒有稅額"
    );
}

/// 號碼格式、統一編號格式、明細數量；買受人是快照，換客戶時重新帶入。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn save_checks_and_buyer_snapshot(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("sales_invoice"),
        &token,
        Some(json!({
            "data": { "invoice_no": "A123456789", "invoice_date": "2026-11-19",
                      "customer_id": m.customer, "tax_type": "taxable", "buyer_tax_id": "1234" },
            "lines": { "sales_invoice_line": [
                { "data": { "description": "A", "qty": "0", "unit_price": "100" } },
            ] },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([
            { "field_key": "invoice_no", "message": "發票號碼格式應為 2 碼英文加 8 碼數字" },
            { "field_key": "buyer_tax_id", "message": "統一編號必須是 8 位數字" },
        ])
    );
    assert_eq!(
        body["details"]["lines"],
        json!([{ "detail_key": "sales_invoice_line", "index": 0, "field_key": "qty",
                 "message": "數量必須大於 0" }])
    );

    let inv = invoice(&pool, &token, m.customer, "AB12345678", &[]).await;
    // 修改客戶資料不影響既有發票
    let (status, _) = call(
        &pool,
        Method::PATCH,
        &record_uri("customer", m.customer),
        &token,
        Some(json!({ "version": 1, "data": { "invoice_title": "改名" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = call(
        &pool,
        Method::GET,
        &record_uri("sales_invoice", inv.id),
        &token,
        None,
    )
    .await;
    assert_eq!(body["data"]["buyer_name"], "大華股份有限公司");

    // 換客戶且沒有帶買受人 → 重新帶入；有帶的以 request 為準
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri("sales_invoice", inv.id),
        &token,
        Some(json!({ "version": inv.version,
                     "data": { "customer_id": m.other_customer, "buyer_address": "手寫地址" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["buyer_name"], "小明商行");
    assert_eq!(body["data"]["buyer_tax_id"], Value::Null);
    assert_eq!(body["data"]["buyer_address"], "手寫地址");
}

/// 關聯出貨單：草稿、其他客戶、重複都不行；開立後同一張出貨單不能再開立；作廢後可以。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn shipment_links(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    let sh = posted_shipment(&pool, &token, m.customer, m.service).await;
    let other = posted_shipment(&pool, &token, m.other_customer, m.service).await;
    let draft = create(
        &pool,
        &token,
        "shipment",
        json!({
            "data": { "customer_id": m.customer, "ship_date": "2026-11-10", "tax_type": "taxable" },
            "lines": { "shipment_line": [
                { "data": { "product_id": m.service, "qty": "1", "unit_price": "1" } },
            ] },
        }),
    )
    .await;

    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("sales_invoice"),
        &token,
        Some(invoice_body(
            m.customer,
            "AB12345678",
            &[draft.id, other.id, sh.id, sh.id],
        )),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let line = |index: usize, message: &str| {
        json!({ "detail_key": "sales_invoice_shipment", "index": index,
                "field_key": "shipment_id", "message": message })
    };
    assert_eq!(
        body["details"]["lines"],
        json!([
            line(0, "出貨單尚未過帳"),
            line(1, "客戶與發票不同"),
            line(3, "出貨單重複"),
        ])
    );

    let a = invoice(&pool, &token, m.customer, "AB12345678", &[sh.id]).await;
    let b = invoice(&pool, &token, m.customer, "AB12345679", &[sh.id]).await;
    let a = post_ok(&pool, &token, "sales_invoice", &a).await;
    assert_eq!(a.data["status"], "posted");
    let message = format!(
        "出貨單 {} 已開立發票 AB12345678",
        sh.record_no.clone().unwrap()
    );

    // 草稿 b 在開立時失敗；新存檔的發票在存檔時就失敗
    let (status, body) = action(&pool, &token, "sales_invoice", b.id, "post", b.version).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["details"]["lines"], json!([line(0, &message)]));
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("sales_invoice"),
        &token,
        Some(invoice_body(m.customer, "AB12345670", &[sh.id])),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["details"]["lines"], json!([line(0, &message)]));

    // 出貨單被有效發票關聯時不可作廢
    let (status, body) = action(&pool, &token, "shipment", sh.id, "void", sh.version).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["message"],
        "此出貨單已開立發票 AB12345678，請先作廢發票"
    );
    assert_eq!(
        body["details"],
        json!({ "fields": [], "documents": [
            { "entity_key": "sales_invoice", "record_id": a.id, "record_no": a.record_no } ] })
    );

    // 作廢發票後，出貨單可以開立在新發票
    let (status, body) = void(
        &pool,
        &token,
        "sales_invoice",
        a.id,
        a.version,
        json!({ "void_reason": "金額開錯", "void_date": "2026-12-01" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["status"], "void");
    assert_eq!(body["data"]["void_date"], "2026-12-01");
    assert_eq!(body["data"]["void_reason"], "金額開錯");
    let b = post_ok(&pool, &token, "sales_invoice", &b).await;

    // 發票作廢後出貨單也可以作廢
    let (status, body) = void(
        &pool,
        &token,
        "sales_invoice",
        b.id,
        b.version,
        json!({ "void_reason": "重開" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = action(&pool, &token, "shipment", sh.id, "void", sh.version).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// 並行開立兩張關聯同一張出貨單的發票：只有一張成功。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn concurrent_posting_links_shipment_once(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    let sh = posted_shipment(&pool, &token, m.customer, m.service).await;
    let a = invoice(&pool, &token, m.customer, "AB12345678", &[sh.id]).await;
    let b = invoice(&pool, &token, m.customer, "AB12345679", &[sh.id]).await;
    let (ra, rb) = tokio::join!(
        action(&pool, &token, "sales_invoice", a.id, "post", 1),
        action(&pool, &token, "sales_invoice", b.id, "post", 1),
    );
    let ok = [&ra, &rb]
        .iter()
        .filter(|(s, body)| {
            assert!(
                *s == StatusCode::OK || *s == StatusCode::UNPROCESSABLE_ENTITY,
                "{s} {body}"
            );
            *s == StatusCode::OK
        })
        .count();
    assert_eq!(ok, 1);
    let posted: i64 =
        sqlx::query_scalar("SELECT count(*) FROM sales_invoices WHERE status = 'posted'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(posted, 1);
}

/// 並行開立同一個號碼：只有一張成功，另一張 422（不會撞上唯一索引成為 500）。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn concurrent_posting_same_number(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    let a = invoice(&pool, &token, m.customer, "AB12345678", &[]).await;
    let b = invoice(&pool, &token, m.customer, "AB12345678", &[]).await;
    let (ra, rb) = tokio::join!(
        action(&pool, &token, "sales_invoice", a.id, "post", 1),
        action(&pool, &token, "sales_invoice", b.id, "post", 1),
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
    assert_eq!(failed["details"]["fields"][0]["field_key"], "invoice_no");
}

/// 開立：號碼不重複（含已作廢）、三聯式買受人必填、至少一列明細。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn posting_checks(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    let a = invoice(&pool, &token, m.customer, "AB12345678", &[]).await;
    let a = post_ok(&pool, &token, "sales_invoice", &a).await;
    let (status, _) = void(
        &pool,
        &token,
        "sales_invoice",
        a.id,
        a.version,
        json!({ "void_reason": "x" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // 已作廢的號碼也不能再用
    let b = invoice(&pool, &token, m.customer, "ab12345678", &[]).await;
    let (status, body) = action(&pool, &token, "sales_invoice", b.id, "post", b.version).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([{ "field_key": "invoice_no",
                 "message": format!("發票號碼 AB12345678 已使用（{}）", a.record_no.clone().unwrap()) }])
    );

    // 三聯式沒有統編、沒有明細
    let c = create(
        &pool,
        &token,
        "sales_invoice",
        json!({ "data": { "invoice_no": "AB12345671", "invoice_date": "2026-11-19",
                          "customer_id": m.other_customer, "tax_type": "taxable" } }),
    )
    .await;
    let (status, body) = action(&pool, &token, "sales_invoice", c.id, "post", c.version).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([{ "field_key": "buyer_tax_id", "message": "三聯式發票必須填寫買受人統一編號" }])
    );
    assert_eq!(
        body["details"]["lines"],
        json!([{ "detail_key": "sales_invoice_line", "index": null, "field_key": null,
                 "message": "至少要有一筆明細" }])
    );

    // 二聯式可以不填統編
    let d = create(
        &pool,
        &token,
        "sales_invoice",
        json!({
            "data": { "invoice_type": "duplicate", "invoice_no": "AB12345672",
                      "invoice_date": "2026-11-19", "customer_id": m.other_customer,
                      "tax_type": "taxable" },
            "lines": { "sales_invoice_line": [
                { "data": { "description": "A", "qty": "1", "unit_price": "105" } },
            ] },
        }),
    )
    .await;
    post_ok(&pool, &token, "sales_invoice", &d).await;
}

/// 作廢：原因必填、日期不可早於發票日期；有效折讓時不可作廢；其他單據不接受作廢日期與原因。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn voiding_rules(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    let inv = invoice(&pool, &token, m.customer, "AB12345678", &[]).await;
    let inv = post_ok(&pool, &token, "sales_invoice", &inv).await;

    let (status, body) = void(
        &pool,
        &token,
        "sales_invoice",
        inv.id,
        inv.version,
        json!({ "void_reason": "  ", "void_date": "2026-11-18" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([
            { "field_key": "void_reason", "message": "請輸入作廢原因" },
            { "field_key": "void_date", "message": "作廢日期不可早於發票日期" },
        ])
    );

    // 有效的折讓（#185 之前以 SQL 模擬）
    let allowance = create(
        &pool,
        &token,
        "sales_allowance",
        json!({
            "data": { "sales_invoice_id": inv.id, "allowance_date": "2026-11-20" },
            "lines": { "sales_allowance_line": [
                { "data": { "description": "折讓", "qty": "1", "unit_price": "10" } },
            ] },
        }),
    )
    .await;
    sqlx::query(
        "UPDATE sales_allowances SET status = 'posted', posted_at = now(),
             posted_by = created_by WHERE id = $1",
    )
    .bind(allowance.id)
    .execute(&pool)
    .await
    .unwrap();
    let (status, body) = void(
        &pool,
        &token,
        "sales_invoice",
        inv.id,
        inv.version,
        json!({ "void_reason": "x" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["message"],
        format!(
            "此發票已有銷貨折讓 {}，請先作廢折讓單",
            allowance.record_no.clone().unwrap()
        )
    );
    assert_eq!(
        body["details"]["documents"][0]["entity_key"],
        "sales_allowance"
    );

    // 沒有帶日期：作廢當天
    sqlx::query("UPDATE sales_allowances SET status = 'draft', posted_at = NULL, posted_by = NULL WHERE id = $1")
        .bind(allowance.id)
        .execute(&pool)
        .await
        .unwrap();
    let (status, body) = void(
        &pool,
        &token,
        "sales_invoice",
        inv.id,
        inv.version,
        json!({ "void_reason": "x" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["data"]["void_date"],
        hammer_shared::time::today_taipei().to_string()
    );

    // 其他單據、其他動作不接受作廢日期與原因
    let sh = posted_shipment(&pool, &token, m.customer, m.service).await;
    let (status, body) = void(
        &pool,
        &token,
        "shipment",
        sh.id,
        sh.version,
        json!({ "void_reason": "x" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([{ "field_key": "void_reason", "message": "此動作不接受作廢日期與原因" }])
    );
    let d = invoice(&pool, &token, m.customer, "AB12345679", &[]).await;
    let (status, _) = call(
        &pool,
        Method::POST,
        &format!("{}/post", record_uri("sales_invoice", d.id)),
        &token,
        Some(json!({ "version": d.version, "void_reason": "x" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

/// 客戶被發票引用時不可刪除。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn customer_with_invoice_cannot_be_deleted(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    invoice(&pool, &token, m.other_customer, "AB12345678", &[]).await;
    let (status, body) = call(
        &pool,
        Method::DELETE,
        &format!("{}?version=1", record_uri("customer", m.other_customer)),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["message"],
        "此客戶已有 1 張發票或折讓單，無法刪除，請改為停用"
    );
    assert_eq!(
        body["details"],
        json!({ "fields": [], "invoice_document_count": 1 })
    );
}

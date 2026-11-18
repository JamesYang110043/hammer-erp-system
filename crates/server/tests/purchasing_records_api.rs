//! M4b 進貨單據的通用資料 API 整合測試（#138，docs/architecture.md §8.8）：採購單、進貨單、進貨退回可以
//! 新增草稿（含明細）並產生單號；系統維護的欄位寫入預設值、request 帶了一律忽略；進貨退回從已過帳的
//! 進貨單帶入廠商、幣別、匯率、稅別與明細的商品、批號、單價。
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

/// 廠商與兩個商品（`goods`、`service`）。
async fn master_data(pool: &PgPool, token: &str) -> (Id, Id, Id) {
    let supplier = create(
        pool,
        token,
        "supplier",
        json!({ "data": { "code": "S001", "name": "美國廠商", "default_currency_code": "USD" } }),
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
    let mut products = Vec::new();
    for (code, product_type) in [("A001", "goods"), ("FEE", "service")] {
        products.push(
            create(
                pool,
                token,
                "product",
                json!({ "data": {
                    "code": code, "name": format!("商品 {code}"), "unit_id": unit,
                    "product_type": product_type,
                } }),
            )
            .await
            .id,
        );
    }
    (supplier, products[0], products[1])
}

fn lines<'a>(record: &'a Record, detail: &str) -> &'a [Record] {
    &record.lines.as_ref().unwrap()[detail]
}

/// 採購單：新增草稿（含明細）產生單號；金額由伺服器計算、已進貨數量為 0；系統維護的欄位帶了忽略。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn purchase_order_draft(pool: PgPool) {
    let (_, token) = editor(&pool).await;
    let (supplier, goods, service) = master_data(&pool, &token).await;

    let po = create(
        &pool,
        &token,
        "purchase_order",
        json!({
            "data": {
                "supplier_id": supplier, "order_date": "2026-11-18", "currency_code": "USD",
                "exchange_rate": "32.15", "tax_type": "zero_rated",
                // 系統維護的欄位：忽略
                "status": "closed", "subtotal": "999", "total_twd": "1",
            },
            "lines": { "purchase_order_line": [
                { "data": { "product_id": goods, "qty": "10", "unit_price": "12.345",
                            "amount": "1", "received_qty": "5" } },
                { "data": { "product_id": service, "qty": "1", "unit_price": "30" } },
            ] },
        }),
    )
    .await;

    let record_no = po.record_no.clone().unwrap();
    assert!(record_no.starts_with("PO20"), "{record_no}");
    assert_eq!(record_no.len(), "PO202611-0001".len(), "{record_no}");
    assert!(
        po.title.starts_with(&format!("{record_no} 美國廠商")),
        "{}",
        po.title
    );
    assert_eq!(po.data["status"], "draft");
    assert_eq!(po.data["exchange_rate"], "32.150000");
    // 美元（小數 2 位）、零稅率：123.45 + 30.00
    assert_eq!(po.data["subtotal"], "153.4500");
    assert_eq!(po.data["tax_amount"], "0.0000");
    assert_eq!(po.data["total"], "153.4500");
    assert_eq!(po.data["total_twd"], "4933.42");
    let po_lines = lines(&po, "purchase_order_line");
    assert_eq!(po_lines.len(), 2);
    assert_eq!(po_lines[0].data["qty"], "10.0000");
    assert_eq!(po_lines[0].data["unit_price"], "12.3450");
    assert_eq!(po_lines[0].data["amount"], "123.4500");
    assert_eq!(po_lines[0].data["received_qty"], "0.0000");
    assert_eq!(po_lines[0].title, "A001 商品 A001");
    let document = po.document.unwrap();
    assert!(document.posted_at.is_none() && document.voided_at.is_none());

    // 第二張的流水號遞增
    let second = create(
        &pool,
        &token,
        "purchase_order",
        json!({ "data": {
            "supplier_id": supplier, "order_date": "2026-11-18", "currency_code": "TWD",
            "tax_type": "taxable",
        } }),
    )
    .await;
    assert_eq!(
        second.record_no.unwrap(),
        format!("{}0002", &record_no[..record_no.len() - 4])
    );
}

/// 進貨單：指向採購單與採購明細（指向系統明細子表的關聯欄位）。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn goods_receipt_draft_refers_to_purchase_order(pool: PgPool) {
    let (_, token) = editor(&pool).await;
    let (supplier, goods, _) = master_data(&pool, &token).await;
    let po = create(
        &pool,
        &token,
        "purchase_order",
        json!({
            "data": { "supplier_id": supplier, "order_date": "2026-11-18", "currency_code": "USD",
                      "tax_type": "zero_rated", "exchange_rate": "32" },
            "lines": { "purchase_order_line": [
                { "data": { "product_id": goods, "qty": "10", "unit_price": "12" } },
            ] },
        }),
    )
    .await;
    let po_line = lines(&po, "purchase_order_line")[0].id;
    // 進貨單只能指向已確認的採購單（#140）
    let (status, body) = call(
        &pool,
        Method::POST,
        &format!("{}/{}/confirm", records_uri("purchase_order"), po.id),
        &token,
        Some(json!({ "version": po.version })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let gr = create(
        &pool,
        &token,
        "goods_receipt",
        json!({
            "data": { "supplier_id": supplier, "purchase_order_id": po.id,
                      "receipt_date": "2026-11-18", "currency_code": "USD", "tax_type": "zero_rated" },
            "lines": { "goods_receipt_line": [
                { "data": { "purchase_order_line_id": po_line, "product_id": goods, "qty": "4",
                            "unit_price": "12", "lot_no": "L-01", "unit_cost_twd": "1" } },
            ] },
        }),
    )
    .await;
    assert!(gr.record_no.as_deref().unwrap().starts_with("GR20"));
    assert_eq!(gr.data["status"], "draft");
    // 匯率可以留空
    assert!(!gr.data.contains_key("exchange_rate"));
    assert_eq!(gr.relations["purchase_order_id"].id, po.id);
    let gr_line = &lines(&gr, "goods_receipt_line")[0];
    assert_eq!(gr_line.data["lot_no"], "L-01");
    assert!(
        !gr_line.data.contains_key("unit_cost_twd"),
        "系統維護的欄位忽略"
    );
    let relation = &gr_line.relations["purchase_order_line_id"];
    assert_eq!(relation.id, po_line);
    assert_eq!(relation.title, "A001 商品 A001");
    assert!(!relation.deleted);

    // 不存在的採購明細 → 422
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("goods_receipt"),
        &token,
        Some(json!({
            "data": { "supplier_id": supplier, "receipt_date": "2026-11-18", "currency_code": "USD",
                      "tax_type": "zero_rated" },
            "lines": { "goods_receipt_line": [
                { "data": { "purchase_order_line_id": Id::now_v7(), "product_id": goods,
                            "qty": "1", "unit_price": "1" } },
            ] },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["lines"][0]["field_key"],
        "purchase_order_line_id"
    );
}

/// 進貨退回：只能退已過帳的進貨單；表頭與明細的系統維護欄位從進貨單帶入；建立後不可更換進貨單。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn purchase_return_copies_receipt(pool: PgPool) {
    let (user, token) = editor(&pool).await;
    let (supplier, goods, service) = master_data(&pool, &token).await;
    let receipt_body = json!({
        "data": { "supplier_id": supplier, "receipt_date": "2026-11-10", "currency_code": "USD",
                  "exchange_rate": "32.15", "tax_type": "zero_rated" },
        "lines": { "goods_receipt_line": [
            { "data": { "product_id": goods, "qty": "10", "unit_price": "12.345", "lot_no": "L-01" } },
            { "data": { "product_id": service, "qty": "1", "unit_price": "30" } },
        ] },
    });
    let gr = create(&pool, &token, "goods_receipt", receipt_body.clone()).await;
    let gr_lines: Vec<Id> = lines(&gr, "goods_receipt_line")
        .iter()
        .map(|l| l.id)
        .collect();
    let return_body = |receipt: Id, line: Id| {
        json!({
            "data": { "goods_receipt_id": receipt, "return_date": "2026-11-18",
                      // 系統維護的欄位：忽略，一律帶入進貨單的值
                      "currency_code": "TWD", "exchange_rate": "1" },
            "lines": { "purchase_return_line": [
                { "data": { "goods_receipt_line_id": line, "qty": "2", "unit_price": "0" } },
            ] },
        })
    };

    // 草稿的進貨單不可退回
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("purchase_return"),
        &token,
        Some(return_body(gr.id, gr_lines[0])),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([{ "field_key": "goods_receipt_id", "message": "進貨單尚未過帳" }])
    );

    // 過帳（#140 之前以 SQL 模擬）
    sqlx::query(
        "UPDATE goods_receipts SET status = 'posted', posted_at = now(), posted_by = $2 WHERE id = $1",
    )
    .bind(gr.id)
    .bind(user)
    .execute(&pool)
    .await
    .unwrap();

    let pr = create(
        &pool,
        &token,
        "purchase_return",
        json!({
            "data": { "goods_receipt_id": gr.id, "return_date": "2026-11-18",
                      "currency_code": "TWD", "exchange_rate": "1" },
            "lines": { "purchase_return_line": [
                { "data": { "goods_receipt_line_id": gr_lines[0], "qty": "2", "unit_price": "0" } },
                { "data": { "goods_receipt_line_id": gr_lines[1], "qty": "1" } },
            ] },
        }),
    )
    .await;
    assert!(pr.record_no.as_deref().unwrap().starts_with("PR20"));
    assert_eq!(pr.data["supplier_id"], json!(supplier));
    assert_eq!(pr.data["currency_code"], "USD");
    assert_eq!(pr.data["exchange_rate"], "32.150000");
    assert_eq!(pr.data["tax_type"], "zero_rated");
    assert_eq!(pr.data["total_twd"], "0.00");
    let pr_lines = lines(&pr, "purchase_return_line");
    assert_eq!(pr_lines[0].data["product_id"], json!(goods));
    assert_eq!(pr_lines[0].data["lot_no"], "L-01");
    assert_eq!(pr_lines[0].data["unit_price"], "12.3450");
    assert_eq!(pr_lines[0].data["qty"], "2.0000");
    assert_eq!(pr_lines[0].title, "A001 商品 A001 L-01");
    assert_eq!(pr_lines[1].data["product_id"], json!(service));
    assert!(!pr_lines[1].data.contains_key("lot_no"));

    // 另一張進貨單的明細 → 422
    let other = create(&pool, &token, "goods_receipt", receipt_body).await;
    let other_line = lines(&other, "goods_receipt_line")[0].id;
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("purchase_return"),
        &token,
        Some(return_body(gr.id, other_line)),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["lines"],
        json!([{ "detail_key": "purchase_return_line", "index": 0,
                 "field_key": "goods_receipt_line_id", "message": "不是此進貨單的明細" }])
    );

    // 修改：不可更換進貨單；其他欄位可以修改，明細重新帶入
    let uri = format!("{}/{}", records_uri("purchase_return"), pr.id);
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &uri,
        &token,
        Some(json!({ "version": pr.version, "data": { "goods_receipt_id": other.id } })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"][0]["message"],
        "建立後不可更換進貨單"
    );
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &uri,
        &token,
        Some(json!({
            "version": pr.version,
            "data": { "reason": "瑕疵" },
            "lines": { "purchase_return_line": [
                { "id": pr_lines[0].id, "data": { "qty": "3" } },
            ] },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let updated: Record = serde_json::from_value(body).unwrap();
    assert_eq!(updated.data["reason"], "瑕疵");
    let updated_lines = lines(&updated, "purchase_return_line");
    assert_eq!(updated_lines.len(), 1);
    assert_eq!(updated_lines[0].data["qty"], "3.0000");
    assert_eq!(updated_lines[0].data["lot_no"], "L-01");
}

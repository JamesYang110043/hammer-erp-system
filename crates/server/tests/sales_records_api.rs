//! M4c 銷貨單據的通用資料 API 整合測試（#146，docs/architecture.md §8.9）：訂單、出貨單、銷貨退回可以
//! 新增草稿（含明細）並產生單號；系統維護的欄位寫入預設值、request 帶了一律忽略；出貨明細可以指定批號；
//! 銷貨退回從已過帳的出貨單帶入客戶、稅別、單價含稅與明細的商品、單價、折扣率。
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

/// 客戶與兩個商品（`goods`、`service`）。
async fn master_data(pool: &PgPool, token: &str) -> (Id, Id, Id) {
    let customer = create(
        pool,
        token,
        "customer",
        json!({ "data": { "code": "C001", "name": "大華公司", "address": "台北市" } }),
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
    (customer, products[0], products[1])
}

fn lines<'a>(record: &'a Record, detail: &str) -> &'a [Record] {
    &record.lines.as_ref().unwrap()[detail]
}

/// 訂單：新增草稿（含明細）產生單號；單價含稅、折扣率有預設值；系統維護的欄位帶了忽略。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn sales_order_draft(pool: PgPool) {
    let (_, token) = editor(&pool).await;
    let (customer, goods, service) = master_data(&pool, &token).await;

    let so = create(
        &pool,
        &token,
        "sales_order",
        json!({
            "data": {
                "customer_id": customer, "order_date": "2026-11-18", "delivery_date": "2026-11-25",
                "tax_type": "taxable",
                // 系統維護的欄位：忽略
                "status": "closed", "subtotal": "999",
            },
            "lines": { "sales_order_line": [
                { "data": { "product_id": goods, "qty": "10", "unit_price": "120",
                            "discount_rate": "0.15", "amount": "1", "shipped_qty": "5" } },
                { "data": { "product_id": service, "qty": "1", "unit_price": "50" } },
            ] },
        }),
    )
    .await;

    let record_no = so.record_no.clone().unwrap();
    assert!(record_no.starts_with("SO20"), "{record_no}");
    assert_eq!(record_no.len(), "SO202611-0001".len(), "{record_no}");
    assert!(
        so.title.starts_with(&format!("{record_no} 大華公司")),
        "{}",
        so.title
    );
    assert_eq!(so.data["status"], "draft");
    assert_eq!(so.data["tax_included"], false);
    // 金額由伺服器計算（#147）：1020 + 50 = 1070；稅額 round(53.5) = 54
    assert_eq!(so.data["subtotal"], "1070.0000");
    assert_eq!(so.data["tax_amount"], "54.0000");
    assert_eq!(so.data["total"], "1124.0000");
    let so_lines = lines(&so, "sales_order_line");
    assert_eq!(so_lines.len(), 2);
    assert_eq!(so_lines[0].data["qty"], "10.0000");
    assert_eq!(so_lines[0].data["discount_rate"], "0.1500");
    assert_eq!(so_lines[0].data["amount"], "1020.0000");
    assert_eq!(so_lines[0].data["shipped_qty"], "0.0000");
    assert_eq!(so_lines[0].title, "A001 商品 A001");
    assert_eq!(so_lines[1].data["discount_rate"], "0.0000");
    let document = so.document.unwrap();
    assert!(document.posted_at.is_none() && document.closed_at.is_none());

    // 折扣率不可大於 1
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("sales_order"),
        &token,
        Some(json!({
            "data": { "customer_id": customer, "order_date": "2026-11-18", "tax_type": "taxable" },
            "lines": { "sales_order_line": [
                { "data": { "product_id": goods, "qty": "1", "unit_price": "1", "discount_rate": "1.5" } },
            ] },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["details"]["lines"][0]["field_key"], "discount_rate");

    // 第二張的流水號遞增
    let second = create(
        &pool,
        &token,
        "sales_order",
        json!({ "data": {
            "customer_id": customer, "order_date": "2026-11-18", "tax_type": "taxable",
            "tax_included": true,
        } }),
    )
    .await;
    assert_eq!(second.data["tax_included"], true);
    assert_eq!(
        second.record_no.unwrap(),
        format!("{}0002", &record_no[..record_no.len() - 4])
    );
}

/// 出貨單：指向訂單與訂單明細（指向系統明細子表的關聯欄位）；明細可以指定批號。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn shipment_draft_refers_to_sales_order(pool: PgPool) {
    let (_, token) = editor(&pool).await;
    let (customer, goods, _) = master_data(&pool, &token).await;
    let so = create(
        &pool,
        &token,
        "sales_order",
        json!({
            "data": { "customer_id": customer, "order_date": "2026-11-18", "tax_type": "taxable" },
            "lines": { "sales_order_line": [
                { "data": { "product_id": goods, "qty": "10", "unit_price": "120" } },
            ] },
        }),
    )
    .await;
    let so_line = lines(&so, "sales_order_line")[0].id;
    // 出貨單存檔時檢查訂單已確認、批號存在（#148）
    let (status, body) = call(
        &pool,
        Method::POST,
        &format!("{}/{}/confirm", records_uri("sales_order"), so.id),
        &token,
        Some(json!({ "version": 1 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let adj = create(
        &pool,
        &token,
        "stock_adjustment",
        json!({
            "data": { "adjustment_date": "2026-11-01", "adjustment_type": "opening" },
            "lines": { "stock_adjustment_line": [
                { "data": { "line_type": "gain_new", "product_id": goods, "lot_no": "L-01",
                            "qty": "10", "unit_cost_twd": "80" } },
            ] },
        }),
    )
    .await;
    let (status, body) = call(
        &pool,
        Method::POST,
        &format!("{}/{}/post", records_uri("stock_adjustment"), adj.id),
        &token,
        Some(json!({ "version": 1 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let sh = create(
        &pool,
        &token,
        "shipment",
        json!({
            "data": { "customer_id": customer, "sales_order_id": so.id, "ship_date": "2026-11-18",
                      "tax_type": "taxable", "ship_address": "台北市", "cost_total_twd": "1" },
            "lines": { "shipment_line": [
                { "data": { "sales_order_line_id": so_line, "product_id": goods, "qty": "4",
                            "unit_price": "120", "lot_no": "L-01", "cost_amount_twd": "1" } },
                { "data": { "sales_order_line_id": so_line, "product_id": goods, "qty": "2",
                            "unit_price": "120" } },
            ] },
        }),
    )
    .await;
    assert!(sh.record_no.as_deref().unwrap().starts_with("SH20"));
    assert_eq!(sh.data["status"], "draft");
    assert_eq!(sh.data["ship_address"], "台北市");
    assert!(
        !sh.data.contains_key("cost_total_twd"),
        "系統維護的欄位忽略"
    );
    assert_eq!(sh.relations["sales_order_id"].id, so.id);
    let sh_lines = lines(&sh, "shipment_line");
    assert_eq!(sh_lines[0].data["lot_no"], "L-01");
    assert_eq!(sh_lines[0].title, "A001 商品 A001 L-01");
    assert!(!sh_lines[0].data.contains_key("cost_amount_twd"));
    assert!(!sh_lines[1].data.contains_key("lot_no"), "留空 = 先進先出");
    let relation = &sh_lines[0].relations["sales_order_line_id"];
    assert_eq!(relation.id, so_line);
    assert_eq!(relation.title, "A001 商品 A001");

    // 不存在的訂單明細 → 422
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("shipment"),
        &token,
        Some(json!({
            "data": { "customer_id": customer, "ship_date": "2026-11-18", "tax_type": "taxable" },
            "lines": { "shipment_line": [
                { "data": { "sales_order_line_id": Id::now_v7(), "product_id": goods,
                            "qty": "1", "unit_price": "1" } },
            ] },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["lines"][0]["field_key"],
        "sales_order_line_id"
    );
}

/// 銷貨退回：只能退已過帳的出貨單；表頭與明細的系統維護欄位從出貨單帶入；建立後不可更換出貨單。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn sales_return_copies_shipment(pool: PgPool) {
    let (user, token) = editor(&pool).await;
    let (customer, goods, service) = master_data(&pool, &token).await;
    let shipment_body = json!({
        "data": { "customer_id": customer, "ship_date": "2026-11-10", "tax_type": "taxable",
                  "tax_included": true },
        "lines": { "shipment_line": [
            { "data": { "product_id": goods, "qty": "10", "unit_price": "105", "discount_rate": "0.1" } },
            { "data": { "product_id": service, "qty": "1", "unit_price": "30" } },
        ] },
    });
    let sh = create(&pool, &token, "shipment", shipment_body.clone()).await;
    let sh_lines: Vec<Id> = lines(&sh, "shipment_line").iter().map(|l| l.id).collect();
    let return_body = |shipment: Id, line: Id| {
        json!({
            "data": { "shipment_id": shipment, "return_date": "2026-11-18",
                      // 系統維護的欄位：忽略，一律帶入出貨單的值
                      "tax_type": "exempt", "tax_included": false },
            "lines": { "sales_return_line": [
                { "data": { "shipment_line_id": line, "qty": "2", "unit_price": "0" } },
            ] },
        })
    };

    // 草稿的出貨單不可退回
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("sales_return"),
        &token,
        Some(return_body(sh.id, sh_lines[0])),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([{ "field_key": "shipment_id", "message": "出貨單尚未過帳" }])
    );

    // 過帳（#148 之前以 SQL 模擬）
    sqlx::query(
        "UPDATE shipments SET status = 'posted', posted_at = now(), posted_by = $2 WHERE id = $1",
    )
    .bind(sh.id)
    .bind(user)
    .execute(&pool)
    .await
    .unwrap();

    let sr = create(
        &pool,
        &token,
        "sales_return",
        json!({
            "data": { "shipment_id": sh.id, "return_date": "2026-11-18", "reason": "瑕疵",
                      "customer_id": Id::now_v7(), "tax_included": false },
            "lines": { "sales_return_line": [
                { "data": { "shipment_line_id": sh_lines[0], "qty": "2", "unit_price": "0",
                            "discount_rate": "0" } },
                { "data": { "shipment_line_id": sh_lines[1], "qty": "1" } },
            ] },
        }),
    )
    .await;
    assert!(sr.record_no.as_deref().unwrap().starts_with("SR20"));
    assert!(sr.title.contains("大華公司"), "{}", sr.title);
    assert_eq!(sr.data["customer_id"], json!(customer));
    assert_eq!(sr.data["tax_type"], "taxable");
    assert_eq!(sr.data["tax_included"], true);
    assert_eq!(sr.data["status"], "draft");
    let sr_lines = lines(&sr, "sales_return_line");
    assert_eq!(sr_lines[0].data["product_id"], json!(goods));
    assert_eq!(sr_lines[0].data["unit_price"], "105.0000");
    assert_eq!(sr_lines[0].data["discount_rate"], "0.1000");
    assert_eq!(sr_lines[0].data["qty"], "2.0000");
    assert_eq!(sr_lines[0].title, "A001 商品 A001");
    assert_eq!(sr_lines[1].data["product_id"], json!(service));
    assert_eq!(sr_lines[1].data["discount_rate"], "0.0000");

    // 另一張出貨單的明細 → 422
    let other = create(&pool, &token, "shipment", shipment_body).await;
    let other_line = lines(&other, "shipment_line")[0].id;
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("sales_return"),
        &token,
        Some(return_body(sh.id, other_line)),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["lines"],
        json!([{ "detail_key": "sales_return_line", "index": 0,
                 "field_key": "shipment_line_id", "message": "不是此出貨單的明細" }])
    );

    // 修改：不可更換出貨單；其他欄位可以修改，明細重新帶入
    let uri = format!("{}/{}", records_uri("sales_return"), sr.id);
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &uri,
        &token,
        Some(json!({ "version": sr.version, "data": { "shipment_id": other.id } })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"][0]["message"],
        "建立後不可更換出貨單"
    );
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &uri,
        &token,
        Some(json!({
            "version": sr.version,
            "data": { "reason": "數量不符" },
            "lines": { "sales_return_line": [
                { "id": sr_lines[0].id, "data": { "qty": "3" } },
            ] },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let updated: Record = serde_json::from_value(body).unwrap();
    assert_eq!(updated.data["reason"], "數量不符");
    let updated_lines = lines(&updated, "sales_return_line");
    assert_eq!(updated_lines.len(), 1);
    assert_eq!(updated_lines[0].data["qty"], "3.0000");
    assert_eq!(updated_lines[0].data["unit_price"], "105.0000");
}

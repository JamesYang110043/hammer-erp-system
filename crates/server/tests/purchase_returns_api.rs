//! 進貨退回的整合測試（#141，docs/architecture.md §8.8「進貨退回」、§9.3）：從進貨單帶入、退回數量上限、
//! 過帳扣指定批號（批號扣除明細、異動）、庫存不足、作廢加回原批號。
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
    goods: Id,
    service: Id,
}

async fn master_data(pool: &PgPool, token: &str) -> Master {
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
    let product = |code: &str, product_type: &str| {
        json!({ "data": { "code": code, "name": format!("商品 {code}"), "unit_id": unit,
                          "product_type": product_type } })
    };
    let goods = create(pool, token, "product", product("A001", "goods"))
        .await
        .id;
    let service = create(pool, token, "product", product("FEE", "service"))
        .await
        .id;
    Master {
        supplier,
        goods,
        service,
    }
}

fn lines<'a>(record: &'a Value, detail: &str) -> &'a Vec<Value> {
    record["lines"][detail].as_array().unwrap()
}

async fn get(pool: &PgPool, token: &str, entity: &str, id: &Value) -> Value {
    let id: Id = serde_json::from_value(id.clone()).unwrap();
    let (status, body) = call(pool, Method::GET, &record_uri(entity, id), token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn create_receipt(pool: &PgPool, token: &str, body: Value) -> (StatusCode, Value) {
    call(
        pool,
        Method::POST,
        &records_uri("goods_receipt"),
        token,
        Some(body),
    )
    .await
}

async fn post_receipt(pool: &PgPool, token: &str, receipt: &Value) -> (StatusCode, Value) {
    let id: Id = serde_json::from_value(receipt["id"].clone()).unwrap();
    let version = i32::try_from(receipt["version"].as_i64().unwrap()).unwrap();
    action(pool, token, "goods_receipt", id, "post", version).await
}

async fn lots(pool: &PgPool, token: &str, product: Id) -> Vec<Value> {
    let (status, body) = call(
        pool,
        Method::GET,
        &format!("/api/v1/inventory/products/{product}/lots?include_empty=true"),
        token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["items"].as_array().unwrap().clone()
}

async fn transactions(pool: &PgPool, token: &str, product: Id) -> Vec<Value> {
    let (status, body) = call(
        pool,
        Method::GET,
        &format!("/api/v1/inventory/transactions?product_id={product}"),
        token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["items"].as_array().unwrap().clone()
}

async fn post_doc(pool: &PgPool, token: &str, entity: &str, doc: &Value) -> (StatusCode, Value) {
    let id: Id = serde_json::from_value(doc["id"].clone()).unwrap();
    let version = i32::try_from(doc["version"].as_i64().unwrap()).unwrap();
    action(pool, token, entity, id, "post", version).await
}

async fn void_doc(pool: &PgPool, token: &str, entity: &str, doc: &Value) -> (StatusCode, Value) {
    let id: Id = serde_json::from_value(doc["id"].clone()).unwrap();
    let version = i32::try_from(doc["version"].as_i64().unwrap()).unwrap();
    action(pool, token, entity, id, "void", version).await
}

/// 已過帳的美元進貨單（2026-11-17、匯率 32.15）：A001 10 個 @ 12.345 批號 L-01、運費 1 @ 7.5。
async fn posted_receipt(pool: &PgPool, token: &str, m: &Master) -> Value {
    let (status, gr) = create_receipt(
        pool,
        token,
        json!({
            "data": { "supplier_id": m.supplier, "receipt_date": "2026-11-17",
                      "exchange_rate": "32.15" },
            "lines": { "goods_receipt_line": [
                { "data": { "product_id": m.goods, "qty": "10", "unit_price": "12.345",
                            "lot_no": "L-01" } },
                { "data": { "product_id": m.service, "qty": "1", "unit_price": "7.5" } },
            ] },
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{gr}");
    let (status, posted) = post_receipt(pool, token, &gr).await;
    assert_eq!(status, StatusCode::OK, "{posted}");
    posted
}

async fn create_return(pool: &PgPool, token: &str, body: Value) -> (StatusCode, Value) {
    call(
        pool,
        Method::POST,
        &records_uri("purchase_return"),
        token,
        Some(body),
    )
    .await
}

fn return_body(gr: &Value, date: &str, lines: &[(usize, &str)]) -> Value {
    let gr_lines = &gr["lines"]["goods_receipt_line"];
    let lines: Vec<Value> = lines
        .iter()
        .map(|(i, qty)| json!({ "data": { "goods_receipt_line_id": gr_lines[*i]["id"], "qty": qty } }))
        .collect();
    json!({
        "data": { "goods_receipt_id": gr["id"], "return_date": date, "reason": "瑕疵" },
        "lines": { "purchase_return_line": lines },
    })
}

/// 過帳扣指定批號、寫入成本與異動；作廢加回原批號。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn post_and_void_return(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    let gr = posted_receipt(&pool, &token, &m).await;

    let (status, ret) = create_return(
        &pool,
        &token,
        return_body(&gr, "2026-11-18", &[(0, "4"), (1, "1")]),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{ret}");
    // 從進貨單帶入；金額依進貨單的幣別、匯率計算
    assert_eq!(ret["data"]["supplier_id"], json!(m.supplier));
    assert_eq!(ret["data"]["exchange_rate"], "32.150000");
    let ret_lines = lines(&ret, "purchase_return_line");
    assert_eq!(ret_lines[0]["data"]["lot_no"], "L-01");
    assert_eq!(ret_lines[0]["data"]["unit_price"], "12.3450");
    assert_eq!(ret_lines[0]["data"]["amount"], "49.3800");
    assert_eq!(ret_lines[1]["data"]["amount"], "7.5000");
    assert_eq!(ret["data"]["subtotal"], "56.8800");
    assert_eq!(ret["data"]["total"], "56.8800");
    assert_eq!(ret["data"]["total_twd"], "1828.69");

    let (status, posted) = post_doc(&pool, &token, "purchase_return", &ret).await;
    assert_eq!(status, StatusCode::OK, "{posted}");
    assert_eq!(posted["data"]["status"], "posted");
    let posted_lines = lines(&posted, "purchase_return_line");
    assert_eq!(posted_lines[0]["data"]["unit_cost_twd"], "396.8918");
    assert_eq!(posted_lines[1]["data"]["unit_cost_twd"], "241.1250");

    let a_lots = lots(&pool, &token, m.goods).await;
    assert_eq!(a_lots[0]["qty_remaining"], "6.0000");
    let record_no = posted["record_no"].as_str().unwrap().to_owned();
    let txns = transactions(&pool, &token, m.goods).await;
    assert_eq!(txns[0]["txn_type"], "purchase_return");
    assert_eq!(txns[0]["qty_change"], "-4.0000");
    assert_eq!(txns[0]["txn_date"], "2026-11-18");
    assert_eq!(txns[0]["unit_cost_twd"], "396.8918");
    assert_eq!(txns[0]["ref_record_no"], record_no);
    let allocations: Vec<(String, String)> = sqlx::query_as(
        "SELECT source_type, qty::text FROM lot_allocations WHERE source_doc_id = $1",
    )
    .bind(posted["id"].as_str().unwrap().parse::<Id>().unwrap())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        allocations,
        [("purchase_return_line".to_owned(), "4.0000".to_owned())]
    );

    // 有已過帳的退回：進貨單不可作廢
    let (status, body) = void_doc(&pool, &token, "goods_receipt", &gr).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["details"]["documents"][0]["record_no"], record_no);

    // 作廢退回：加回原批號
    let (status, voided) = void_doc(&pool, &token, "purchase_return", &posted).await;
    assert_eq!(status, StatusCode::OK, "{voided}");
    assert_eq!(voided["data"]["status"], "void");
    assert_eq!(
        lots(&pool, &token, m.goods).await[0]["qty_remaining"],
        "10.0000"
    );
    let txns = transactions(&pool, &token, m.goods).await;
    assert_eq!(txns[0]["txn_type"], "void_reversal");
    assert_eq!(txns[0]["qty_change"], "4.0000");
    assert_eq!(txns[0]["ref_record_no"], record_no);

    // 退回作廢後，進貨單可以作廢
    let (status, body) = void_doc(&pool, &token, "goods_receipt", &gr).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// 退回數量上限：超過可退數量（扣除其他已過帳的退回）、重複的進貨明細、退回日期。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn return_quantity_limits(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    let gr = posted_receipt(&pool, &token, &m).await;

    let (status, body) = create_return(
        &pool,
        &token,
        return_body(&gr, "2026-11-16", &[(0, "11"), (0, "1")]),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([{ "field_key": "return_date", "message": "退回日期不可早於進貨日期" }])
    );
    let got: Vec<(i64, String, String)> = body["details"]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["index"].as_i64().unwrap(),
                e["field_key"].as_str().unwrap().to_owned(),
                e["message"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            (0, "qty".to_owned(), "超過可退數量 10.0000".to_owned()),
            (
                1,
                "goods_receipt_line_id".to_owned(),
                "進貨明細重複".to_owned()
            ),
        ]
    );

    // 兩張草稿：先過帳 6 的成功，之後可退 4
    let (_, first) =
        create_return(&pool, &token, return_body(&gr, "2026-11-18", &[(0, "6")])).await;
    let (status, second) =
        create_return(&pool, &token, return_body(&gr, "2026-11-18", &[(0, "6")])).await;
    assert_eq!(status, StatusCode::CREATED, "{second}");
    let (status, _) = post_doc(&pool, &token, "purchase_return", &first).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = post_doc(&pool, &token, "purchase_return", &second).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["lines"][0]["message"],
        "超過可退數量 4.0000"
    );
    let (status, body) = create_return(
        &pool,
        &token,
        return_body(&gr, "2026-11-18", &[(0, "4.0001")]),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["lines"][0]["message"],
        "超過可退數量 4.0000"
    );

    // 草稿的進貨單不能退
    let (_, draft) = create_receipt(
        &pool,
        &token,
        json!({
            "data": { "supplier_id": m.supplier, "receipt_date": "2026-11-17",
                      "exchange_rate": "32" },
            "lines": { "goods_receipt_line": [
                { "data": { "product_id": m.goods, "qty": "1", "unit_price": "1" } },
            ] },
        }),
    )
    .await;
    let (status, body) = create_return(
        &pool,
        &token,
        return_body(&draft, "2026-11-18", &[(0, "1")]),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([{ "field_key": "goods_receipt_id", "message": "進貨單尚未過帳" }])
    );
}

/// 批號剩餘數量不足：過帳時 STOCK_INSUFFICIENT，整張回復。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn return_exceeding_lot_remaining_is_rejected(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    let gr = posted_receipt(&pool, &token, &m).await;

    // 盤虧 8 個，批號剩 2
    let adjustment = create(
        &pool,
        &token,
        "stock_adjustment",
        json!({
            "data": { "adjustment_date": "2026-11-18" },
            "lines": { "stock_adjustment_line": [
                { "data": { "product_id": m.goods, "line_type": "loss", "lot_no": "L-01",
                            "qty": "8" } },
            ] },
        }),
    )
    .await;
    let (status, _) = action(
        &pool,
        &token,
        "stock_adjustment",
        adjustment.id,
        "post",
        adjustment.version,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // 存檔時不檢查庫存
    let (status, ret) = create_return(
        &pool,
        &token,
        return_body(&gr, "2026-11-19", &[(0, "3"), (1, "1")]),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{ret}");
    let (status, body) = post_doc(&pool, &token, "purchase_return", &ret).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "STOCK_INSUFFICIENT");
    let item = &body["details"]["items"][0];
    assert_eq!(item["lot_no"], "L-01");
    assert_eq!(item["required"], "3.0000");
    assert_eq!(item["available"], "2.0000");
    assert_eq!(item["shortage"], "1.0000");
    assert_eq!(
        item["lines"],
        json!([{ "detail_key": "purchase_return_line", "index": 0 }])
    );

    // 完全回復：仍是草稿，批號數量不變，沒有退回的異動
    let ret_now = get(&pool, &token, "purchase_return", &ret["id"]).await;
    assert_eq!(ret_now["data"]["status"], "draft");
    assert_eq!(ret_now["version"], ret["version"]);
    assert_eq!(
        lots(&pool, &token, m.goods).await[0]["qty_remaining"],
        "2.0000"
    );
    let txns = transactions(&pool, &token, m.goods).await;
    assert!(txns.iter().all(|t| t["ref_doc_type"] != "purchase_return"));

    // 改為 2 個可以過帳
    let id: Id = serde_json::from_value(ret["id"].clone()).unwrap();
    let line_id = ret["lines"]["purchase_return_line"][0]["id"].clone();
    let service_line = ret["lines"]["purchase_return_line"][1]["id"].clone();
    let (status, updated) = call(
        &pool,
        Method::PATCH,
        &record_uri("purchase_return", id),
        &token,
        Some(json!({
            "version": ret["version"],
            "lines": { "purchase_return_line": [
                { "id": line_id, "data": { "qty": "2" } },
                { "id": service_line, "data": {} },
            ] },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["data"]["subtotal"], "32.1900");
    let (status, body) = post_doc(&pool, &token, "purchase_return", &updated).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        lots(&pool, &token, m.goods).await[0]["qty_remaining"],
        "0.0000"
    );
}

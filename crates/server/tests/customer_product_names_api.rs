//! 客戶品名的整合測試（#265，docs/architecture.md §8.7 `customer_product_names`、§8.9「明細的品名」）：
//! 透過客戶的通用資料 API 維護客戶品名明細與檢查；訂單、出貨單、銷貨退回存檔時品名的帶入。
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

async fn user(pool: &PgPool) -> String {
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

async fn create(pool: &PgPool, token: &str, entity: &str, body: Value) -> Record {
    let (status, body) = call(pool, Method::POST, &records_uri(entity), token, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn patch(pool: &PgPool, token: &str, entity: &str, id: Id, body: Value) -> Record {
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

fn lines<'a>(record: &'a Record, detail: &str) -> &'a [Record] {
    &record.lines.as_ref().unwrap()[detail]
}

fn names(record: &Record, detail: &str) -> Vec<String> {
    lines(record, detail)
        .iter()
        .map(|l| l.data["product_name"].as_str().unwrap().to_owned())
        .collect()
}

struct Master {
    /// 有客戶品名（P001 → X 型扳手）的客戶。
    a: Id,
    /// 沒有客戶品名的客戶。
    b: Id,
    /// 服務類商品（不需要庫存即可過帳）：扳手 10mm、鉗子。
    p1: Id,
    p2: Id,
}

async fn master_data(pool: &PgPool, token: &str) -> Master {
    let unit = create(
        pool,
        token,
        "unit",
        json!({ "data": { "code": "pcs", "name": "個" } }),
    )
    .await
    .id;
    let product = |code: &str, name: &str| {
        json!({ "data": { "code": code, "name": name, "unit_id": unit,
                          "product_type": "service" } })
    };
    let p1 = create(pool, token, "product", product("P001", "扳手 10mm"))
        .await
        .id;
    let p2 = create(pool, token, "product", product("P002", "鉗子"))
        .await
        .id;
    let a = create(
        pool,
        token,
        "customer",
        json!({
            "data": { "code": "C001", "name": "客戶 A" },
            "lines": { "customer_product_name": [
                { "data": { "product_id": p1, "name": "X 型扳手" } },
            ] },
        }),
    )
    .await
    .id;
    let b = create(
        pool,
        token,
        "customer",
        json!({ "data": { "code": "C002", "name": "客戶 B" } }),
    )
    .await
    .id;
    Master { a, b, p1, p2 }
}

fn order_body(customer: Id, lines: Vec<Value>) -> Value {
    json!({
        "data": { "customer_id": customer, "order_date": "2026-11-25" },
        "lines": { "sales_order_line": lines },
    })
}

fn line(product: Id) -> Value {
    json!({ "data": { "product_id": product, "qty": "1", "unit_price": "100" } })
}

fn named_line(product: Id, name: &str) -> Value {
    json!({ "data": { "product_id": product, "product_name": name, "qty": "1",
                      "unit_price": "100" } })
}

/// 客戶品名透過客戶的通用資料 API 新增、修改、刪除；名稱去除前後空白。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn maintain_customer_product_names(pool: PgPool) {
    let token = user(&pool).await;
    let m = master_data(&pool, &token).await;

    let (status, body) = call(
        &pool,
        Method::GET,
        &record_uri("customer", m.a),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let customer: Record = serde_json::from_value(body).unwrap();
    let rows = lines(&customer, "customer_product_name");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].data["name"], "X 型扳手");
    assert_eq!(rows[0].title, "P001 X 型扳手");
    let first = rows[0].id;

    // 修改第一列、新增第二列（名稱去除前後空白）
    let customer = patch(
        &pool,
        &token,
        "customer",
        m.a,
        json!({
            "version": customer.version,
            "data": {},
            "lines": { "customer_product_name": [
                { "id": first, "data": { "name": "X 型扳手（新）" } },
                { "data": { "product_id": m.p2, "name": "  工具組零件 03 ", "notes": "備註" } },
            ] },
        }),
    )
    .await;
    let rows = lines(&customer, "customer_product_name");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, first);
    assert_eq!(rows[0].data["name"], "X 型扳手（新）");
    assert_eq!(rows[1].data["name"], "工具組零件 03");

    // 刪除第一列；之後同一個商品可以再新增
    let customer = patch(
        &pool,
        &token,
        "customer",
        m.a,
        json!({
            "version": customer.version,
            "data": {},
            "lines": { "customer_product_name": [ { "id": rows[1].id, "data": {} } ] },
        }),
    )
    .await;
    assert_eq!(lines(&customer, "customer_product_name").len(), 1);
    let customer = patch(
        &pool,
        &token,
        "customer",
        m.a,
        json!({
            "version": customer.version,
            "data": {},
            "lines": { "customer_product_name": [
                { "id": rows[1].id, "data": {} },
                { "data": { "product_id": m.p1, "name": "Y 型扳手" } },
            ] },
        }),
    )
    .await;
    assert_eq!(lines(&customer, "customer_product_name").len(), 2);

    // 同一次存檔中刪除一列、再以同一個商品新增（先刪除才寫入，不會違反唯一索引）
    let rows = lines(&customer, "customer_product_name");
    let customer = patch(
        &pool,
        &token,
        "customer",
        m.a,
        json!({
            "version": customer.version,
            "data": {},
            "lines": { "customer_product_name": [
                { "id": rows[0].id, "data": {} },
                { "data": { "product_id": m.p1, "name": "Z 型扳手" } },
            ] },
        }),
    )
    .await;
    let rows = lines(&customer, "customer_product_name");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].data["name"], "Z 型扳手");
}

/// 客戶品名的檢查：同一商品重複、名稱空白或有控制字元、商品不存在。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn customer_product_name_checks(pool: PgPool) {
    let token = user(&pool).await;
    let m = master_data(&pool, &token).await;
    let (status, body) = call(
        &pool,
        Method::GET,
        &record_uri("customer", m.b),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let version = body["version"].clone();

    let save = |lines: Value| {
        json!({ "version": version, "data": {},
                "lines": { "customer_product_name": lines } })
    };

    // 同一次存檔中同一商品重複 → 第二列
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri("customer", m.b),
        &token,
        Some(save(json!([
            { "data": { "product_id": m.p1, "name": "A" } },
            { "data": { "product_id": m.p2, "name": "B" } },
            { "data": { "product_id": m.p1, "name": "C" } },
        ]))),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["lines"],
        json!([{ "detail_key": "customer_product_name", "index": 2,
                 "field_key": "product_id", "message": "此商品已設定客戶品名" }])
    );

    // 名稱空白、控制字元、商品不存在
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri("customer", m.b),
        &token,
        Some(save(json!([
            { "data": { "product_id": m.p1, "name": "   " } },
            { "data": { "product_id": m.p2, "name": "X\u{7}Y" } },
            { "data": { "product_id": Id::now_v7(), "name": "Z" } },
        ]))),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let errors = body["details"]["lines"].as_array().unwrap();
    let at = |index: u64| {
        errors
            .iter()
            .find(|e| e["index"] == index)
            .unwrap_or_else(|| panic!("第 {index} 列沒有錯誤：{body}"))
    };
    assert_eq!(at(0)["field_key"], "name");
    assert_eq!(at(0)["message"], "此欄位為必填");
    assert_eq!(at(1)["field_key"], "name");
    assert_eq!(at(1)["message"], "客戶品名不可包含控制字元");
    assert_eq!(at(2)["field_key"], "product_id");

    // 名稱超過 200 字
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri("customer", m.b),
        &token,
        Some(save(json!([
            { "data": { "product_id": m.p1, "name": "名".repeat(201) } },
        ]))),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["details"]["lines"][0]["field_key"], "name");
}

/// 訂單的品名帶入：客戶品名 → 商品名稱；帶了品名以帶入的為準；修改時換商品重新帶入、只改其他欄位保留。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn sales_order_product_names(pool: PgPool) {
    let token = user(&pool).await;
    let m = master_data(&pool, &token).await;

    // 客戶 A：P001 有客戶品名、P002 沒有；帶了品名時以帶入的為準（去除前後空白）
    let so = create(
        &pool,
        &token,
        "sales_order",
        order_body(
            m.a,
            vec![line(m.p1), line(m.p2), named_line(m.p1, "  特製扳手 ")],
        ),
    )
    .await;
    assert_eq!(
        names(&so, "sales_order_line"),
        ["X 型扳手", "鉗子", "特製扳手"]
    );
    // 客戶 B 沒有設定客戶品名
    let so_b = create(
        &pool,
        &token,
        "sales_order",
        order_body(m.b, vec![line(m.p1)]),
    )
    .await;
    assert_eq!(names(&so_b, "sales_order_line"), ["扳手 10mm"]);

    // 修改：第一列只改數量 → 保留；第二列換商品 → 重新帶入；第三列帶空白 → 重新帶入
    let rows = lines(&so, "sales_order_line");
    let so = patch(
        &pool,
        &token,
        "sales_order",
        so.id,
        json!({
            "version": so.version,
            "data": {},
            "lines": { "sales_order_line": [
                { "id": rows[0].id, "data": { "qty": "5" } },
                { "id": rows[1].id, "data": { "product_id": m.p1 } },
                { "id": rows[2].id, "data": { "product_name": " " } },
            ] },
        }),
    )
    .await;
    assert_eq!(
        names(&so, "sales_order_line"),
        ["X 型扳手", "X 型扳手", "X 型扳手"]
    );

    // 手動修改的品名在只改其他欄位時保留
    let rows = lines(&so, "sales_order_line");
    let so = patch(
        &pool,
        &token,
        "sales_order",
        so.id,
        json!({
            "version": so.version,
            "data": {},
            "lines": { "sales_order_line": [
                { "id": rows[0].id, "data": { "product_name": "手改品名" } },
                { "id": rows[1].id, "data": {} },
                { "id": rows[2].id, "data": {} },
            ] },
        }),
    )
    .await;
    let rows = lines(&so, "sales_order_line");
    let so = patch(
        &pool,
        &token,
        "sales_order",
        so.id,
        json!({
            "version": so.version,
            "data": { "notes": "只改表頭" },
            "lines": { "sales_order_line": [
                { "id": rows[0].id, "data": { "unit_price": "90" } },
                { "id": rows[1].id, "data": {} },
                { "id": rows[2].id, "data": {} },
            ] },
        }),
    )
    .await;
    assert_eq!(names(&so, "sales_order_line")[0], "手改品名");

    // 表頭換客戶：不重新帶入既有列的品名；新增的列依新客戶帶入
    let rows = lines(&so, "sales_order_line");
    let so = patch(
        &pool,
        &token,
        "sales_order",
        so.id,
        json!({
            "version": so.version,
            "data": { "customer_id": m.b },
            "lines": { "sales_order_line": [
                { "id": rows[0].id, "data": {} },
                { "id": rows[1].id, "data": {} },
                { "id": rows[2].id, "data": {} },
                line(m.p1),
            ] },
        }),
    )
    .await;
    assert_eq!(
        names(&so, "sales_order_line"),
        ["手改品名", "X 型扳手", "X 型扳手", "扳手 10mm"]
    );

    // 品名有控制字元 → 該列的錯誤
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("sales_order"),
        &token,
        Some(order_body(m.a, vec![named_line(m.p1, "X\u{7}")])),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["lines"],
        json!([{ "detail_key": "sales_order_line", "index": 0,
                 "field_key": "product_name", "message": "品名不可包含控制字元" }])
    );
}

/// 訂單轉出貨單沿用訂單明細的品名；直接出貨依客戶品名；銷貨退回 = 出貨明細的品名；
/// 之後修改客戶品名或商品名稱，既有單據不變。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn shipment_and_return_product_names(pool: PgPool) {
    let token = user(&pool).await;
    let m = master_data(&pool, &token).await;

    let so = create(
        &pool,
        &token,
        "sales_order",
        order_body(m.a, vec![named_line(m.p1, "特製扳手"), line(m.p2)]),
    )
    .await;
    let so = {
        let (status, body) = call(
            &pool,
            Method::POST,
            &format!("{}/confirm", record_uri("sales_order", so.id)),
            &token,
            Some(json!({ "version": so.version })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        serde_json::from_value::<Record>(body).unwrap()
    };
    let so_lines = lines(&so, "sales_order_line");

    // 出貨明細：有訂單明細 → 訂單明細的品名；沒有 → 客戶品名
    let sh = create(
        &pool,
        &token,
        "shipment",
        json!({
            "data": { "customer_id": m.a, "sales_order_id": so.id, "ship_date": "2026-11-25" },
            "lines": { "shipment_line": [
                { "data": { "sales_order_line_id": so_lines[0].id, "product_id": m.p1,
                            "qty": "1", "unit_price": "100" } },
                { "data": { "sales_order_line_id": so_lines[1].id, "product_id": m.p2,
                            "qty": "1", "unit_price": "100" } },
                line(m.p1),
            ] },
        }),
    )
    .await;
    assert_eq!(
        names(&sh, "shipment_line"),
        ["特製扳手", "鉗子", "X 型扳手"]
    );

    let sh = {
        let (status, body) = call(
            &pool,
            Method::POST,
            &format!("{}/post", record_uri("shipment", sh.id)),
            &token,
            Some(json!({ "version": sh.version })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        serde_json::from_value::<Record>(body).unwrap()
    };
    let sh_lines = lines(&sh, "shipment_line");

    // 銷貨退回：品名 = 出貨明細的品名，request 帶了也忽略（系統維護）
    let sr = create(
        &pool,
        &token,
        "sales_return",
        json!({
            "data": { "shipment_id": sh.id, "return_date": "2026-11-26" },
            "lines": { "sales_return_line": [
                { "data": { "shipment_line_id": sh_lines[0].id, "qty": "1",
                            "product_name": "改不了" } },
                { "data": { "shipment_line_id": sh_lines[2].id, "qty": "1" } },
            ] },
        }),
    )
    .await;
    assert_eq!(names(&sr, "sales_return_line"), ["特製扳手", "X 型扳手"]);

    // 修改客戶品名與商品名稱後，既有單據的品名不變
    let (_, customer) = call(
        &pool,
        Method::GET,
        &record_uri("customer", m.a),
        &token,
        None,
    )
    .await;
    let row = customer["lines"]["customer_product_name"][0]["id"].clone();
    patch(
        &pool,
        &token,
        "customer",
        m.a,
        json!({
            "version": customer["version"],
            "data": {},
            "lines": { "customer_product_name": [
                { "id": row, "data": { "name": "Y 型扳手" } },
            ] },
        }),
    )
    .await;
    let (_, product) = call(
        &pool,
        Method::GET,
        &record_uri("product", m.p2),
        &token,
        None,
    )
    .await;
    patch(
        &pool,
        &token,
        "product",
        m.p2,
        json!({ "version": product["version"], "data": { "name": "尖嘴鉗" } }),
    )
    .await;

    for (entity, id, detail, expected) in [
        (
            "sales_order",
            so.id,
            "sales_order_line",
            vec!["特製扳手", "鉗子"],
        ),
        (
            "shipment",
            sh.id,
            "shipment_line",
            vec!["特製扳手", "鉗子", "X 型扳手"],
        ),
        (
            "sales_return",
            sr.id,
            "sales_return_line",
            vec!["特製扳手", "X 型扳手"],
        ),
    ] {
        let (status, body) = call(&pool, Method::GET, &record_uri(entity, id), &token, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let record: Record = serde_json::from_value(body).unwrap();
        assert_eq!(names(&record, detail), expected, "{entity}");
    }

    // 之後新開的單據使用新的客戶品名與商品名稱
    let so = create(
        &pool,
        &token,
        "sales_order",
        order_body(m.a, vec![line(m.p1), line(m.p2)]),
    )
    .await;
    assert_eq!(names(&so, "sales_order_line"), ["Y 型扳手", "尖嘴鉗"]);
}

/// `relations` 的 `inactive`（#280，§7.3）：指向停用的商品、客戶時為 `true`；已刪除時為 `false`
/// （`deleted = true`）；沒有 `is_active` 的資料類別（單位）一律 `false`。存檔不受影響。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn relation_inactive_flag(pool: PgPool) {
    let token = user(&pool).await;
    let m = master_data(&pool, &token).await;
    let get = |entity: &'static str, id: Id| {
        let pool = pool.clone();
        let token = token.clone();
        async move {
            let (status, body) =
                call(&pool, Method::GET, &record_uri(entity, id), &token, None).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            serde_json::from_value::<Record>(body).unwrap()
        }
    };
    let product_ref = |customer: &Record| {
        lines(customer, "customer_product_name")[0].relations["product_id"].clone()
    };

    let customer = get("customer", m.a).await;
    let r = product_ref(&customer);
    assert!(!r.inactive && !r.deleted);

    // 停用商品 → 客戶品名的商品 inactive；修改客戶品名照常存檔
    let product = get("product", m.p1).await;
    assert!(!product.relations["unit_id"].inactive);
    patch(
        &pool,
        &token,
        "product",
        m.p1,
        json!({ "version": product.version, "data": { "is_active": false } }),
    )
    .await;
    let customer = get("customer", m.a).await;
    let r = product_ref(&customer);
    assert!(r.inactive && !r.deleted, "{r:?}");
    let row = lines(&customer, "customer_product_name")[0].id;
    let customer = patch(
        &pool,
        &token,
        "customer",
        m.a,
        json!({ "version": customer.version, "data": {},
                "lines": { "customer_product_name": [
                    { "id": row, "data": { "name": "停用後改名" } } ] } }),
    )
    .await;
    assert!(product_ref(&customer).inactive);

    // 單據的表頭與其他明細表：停用的客戶、商品
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("sales_order"),
        &token,
        Some(order_body(m.b, vec![line(m.p1), line(m.p2)])),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let order: Record = serde_json::from_value(body).unwrap();
    assert!(!order.relations["customer_id"].inactive);
    let order_lines = lines(&order, "sales_order_line");
    assert!(order_lines[0].relations["product_id"].inactive);
    assert!(!order_lines[1].relations["product_id"].inactive);
    sqlx::query("UPDATE customers SET is_active = false WHERE id = $1")
        .bind(m.b)
        .execute(&pool)
        .await
        .unwrap();
    let order = get("sales_order", order.id).await;
    assert!(order.relations["customer_id"].inactive);

    // 已刪除：deleted = true、inactive = false
    sqlx::query("UPDATE products SET deleted_at = now() WHERE id = $1")
        .bind(m.p1)
        .execute(&pool)
        .await
        .unwrap();
    let r = product_ref(&get("customer", m.a).await);
    assert!(r.deleted && !r.inactive, "{r:?}");
}

/// 商品的客戶品名（#281，§8.7「商品畫面的客戶品名」）：依客戶編號排序；已刪除的客戶、客戶品名不出現，
/// 停用的客戶出現；商品不存在或已刪除 → 404；沒有 `record.read` → 403。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn customer_names_of_product(pool: PgPool) {
    let token = user(&pool).await;
    let m = master_data(&pool, &token).await;
    let uri = |product: Id| format!("/api/v1/products/{product}/customer-product-names");

    // 依客戶編號排序：C000（後建立）在 C001 之前；C002 停用；C003 已刪除；C004 的客戶品名已刪除
    let cpn = |name: &str, notes: Option<&str>| json!({ "data": { "product_id": m.p1, "name": name, "notes": notes } });
    let c0 = create(
        &pool,
        &token,
        "customer",
        json!({ "data": { "code": "C000", "name": "客戶零" },
                "lines": { "customer_product_name": [ cpn("零號扳手", Some("圖面 W-0")) ] } }),
    )
    .await;
    let b = patch(
        &pool,
        &token,
        "customer",
        m.b,
        json!({ "version": 1, "data": { "is_active": false },
                "lines": { "customer_product_name": [ cpn("B 的扳手", None) ] } }),
    )
    .await;
    let c3 = create(
        &pool,
        &token,
        "customer",
        json!({ "data": { "code": "C003", "name": "已刪除的客戶" },
                "lines": { "customer_product_name": [ cpn("刪除客戶的扳手", None) ] } }),
    )
    .await;
    sqlx::query("UPDATE customers SET deleted_at = now() WHERE id = $1")
        .bind(c3.id)
        .execute(&pool)
        .await
        .unwrap();
    let c4 = create(
        &pool,
        &token,
        "customer",
        json!({ "data": { "code": "C004", "name": "客戶四" },
                "lines": { "customer_product_name": [ cpn("會被刪除", None) ] } }),
    )
    .await;
    patch(
        &pool,
        &token,
        "customer",
        c4.id,
        json!({ "version": c4.version, "data": {}, "lines": { "customer_product_name": [] } }),
    )
    .await;

    let (status, body) = call(&pool, Method::GET, &uri(m.p1), &token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = body.as_array().unwrap();
    let summary: Vec<(&str, &str, bool)> = rows
        .iter()
        .map(|r| {
            (
                r["customer_title"].as_str().unwrap(),
                r["name"].as_str().unwrap(),
                r["customer_is_active"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("C000 客戶零", "零號扳手", true),
            ("C001 客戶 A", "X 型扳手", true),
            ("C002 客戶 B", "B 的扳手", false),
        ]
    );
    assert_eq!(rows[0]["customer_id"], json!(c0.id));
    assert_eq!(rows[0]["notes"], "圖面 W-0");
    assert_eq!(rows[2]["notes"], Value::Null);
    assert_eq!(
        rows[2]["id"],
        json!(lines(&b, "customer_product_name")[0].id)
    );

    // 沒有客戶品名的商品：空陣列；停用的商品照常回傳
    let (status, body) = call(&pool, Method::GET, &uri(m.p2), &token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!([]));
    sqlx::query("UPDATE products SET is_active = false WHERE id = $1")
        .bind(m.p1)
        .execute(&pool)
        .await
        .unwrap();
    let (status, body) = call(&pool, Method::GET, &uri(m.p1), &token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.as_array().unwrap().len(), 3);

    // 商品不存在、已刪除 → 404
    let (status, body) = call(&pool, Method::GET, &uri(Id::now_v7()), &token, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["message"], "找不到此商品");
    sqlx::query("UPDATE products SET deleted_at = now() WHERE id = $1")
        .bind(m.p1)
        .execute(&pool)
        .await
        .unwrap();
    let (status, body) = call(&pool, Method::GET, &uri(m.p1), &token, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // 沒有 record.read → 403
    sqlx::query(
        "DELETE FROM role_permissions WHERE permission = 'record.read'
           AND role_id = (SELECT id FROM roles WHERE code = 'editor')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let (status, body) = call(&pool, Method::GET, &uri(m.p2), &token, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

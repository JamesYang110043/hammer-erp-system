//! 庫存查詢 API 的整合測試（#132，docs/architecture.md §8.10「庫存查詢 API」、§10）：商品庫存、商品的
//! 批號、異動流水帳的篩選、排序、分頁與格式，以及權限（唯讀者可以查詢、未登入 `401`）。
//!
//! 庫存資料以盤點調整單的過帳、作廢建立。
//!
//! 使用 `#[sqlx::test]`，需要 `DATABASE_URL`（見 README「整合測試」）。

#![allow(clippy::unwrap_used)]

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use hammer_core::test_util::{TestUser, create_user_with_session};
use hammer_server::{AppState, Config, router};
use hammer_shared::inventory::{InventoryLot, InventoryTransaction, StockBalance};
use hammer_shared::pagination::Page;
use hammer_shared::record::Record;
use hammer_shared::time::today_taipei;
use hammer_shared::{ApiError, ErrorCode, Id, role};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

const DOC: &str = "stock_adjustment";
const LINE: &str = "stock_adjustment_line";

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
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::USER_AGENT, "HammerERP-test")
        .header("X-Client-Version", "0.1.0");
    if let Some(token) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
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

async fn token_for(pool: &PgPool, username: &str, role_code: &str) -> String {
    let (_, token) = create_user_with_session(pool, &TestUser::new(username, role_code))
        .await
        .unwrap();
    token.into_string()
}

async fn create(pool: &PgPool, token: &str, entity: &str, body: Value) -> Record {
    let (status, body) = call(
        pool,
        Method::POST,
        &format!("/api/v1/entities/{entity}/records"),
        Some(token),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn act(pool: &PgPool, token: &str, record: &Record, action: &str) -> Record {
    let (status, body) = call(
        pool,
        Method::POST,
        &format!("/api/v1/entities/{DOC}/records/{}/{action}", record.id),
        Some(token),
        Some(json!({ "version": record.version })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

/// 過帳一張盤點調整單。
async fn posted(pool: &PgPool, token: &str, date: &str, kind: &str, lines: Value) -> Record {
    let record = create(
        pool,
        token,
        DOC,
        json!({
            "data": { "adjustment_date": date, "adjustment_type": kind },
            "lines": { LINE: lines },
        }),
    )
    .await;
    act(pool, token, &record, "post").await
}

async fn get_page<T: serde::de::DeserializeOwned>(
    pool: &PgPool,
    token: &str,
    uri: &str,
) -> Page<T> {
    let (status, body) = call(pool, Method::GET, uri, Some(token), None).await;
    assert_eq!(status, StatusCode::OK, "{uri} {body}");
    serde_json::from_value(body).unwrap()
}

fn assert_invalid_param(status: StatusCode, body: &Value, field: &str) {
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let err: ApiError = serde_json::from_value(body.clone()).unwrap();
    assert_eq!(err.code, ErrorCode::Validation, "{body}");
    assert_eq!(err.details, Some(json!({ "field": field })), "{body}");
}

/// 測試資料：
///
/// - `b-002` 螺絲 M3（分類「五金」、安全庫存 20）：期初 `B-OLD` 10 @ 12.5（11/01）、`B-NEW` 4 @ 10.0005
///   （11/05）；盤虧 `B-OLD` 10（11/06，用完）
/// - `A001` 螺帽 M3（停用、安全庫存 1）：沒有批號
/// - `C003` 墊片：期初 `C-01` 2.5 @ 100.1234（11/01）；之後作廢一張盤盈 `C-X`
/// - `S001` 安裝費（服務）、`D004` 已刪除的商品：不出現在庫存
struct Fixture {
    editor: String,
    b: Id,
    a: Id,
    c: Id,
    service: Id,
    deleted: Id,
    category: Id,
    opening: Record,
    later: Record,
    loss: Record,
    voided: Record,
}

async fn fixture(pool: &PgPool) -> Fixture {
    let editor = token_for(pool, "editor", role::EDITOR).await;
    let unit = create(
        pool,
        &editor,
        "unit",
        json!({ "data": { "code": "pcs", "name": "個" } }),
    )
    .await
    .id;
    let category = create(
        pool,
        &editor,
        "product_category",
        json!({ "data": { "name": "五金" } }),
    )
    .await
    .id;
    let product = |data: Value| {
        let editor = editor.clone();
        async move {
            let mut data = data;
            data["unit_id"] = json!(unit);
            create(pool, &editor, "product", json!({ "data": data }))
                .await
                .id
        }
    };
    let b = product(json!({ "code": "b-002", "name": "螺絲 M3", "spec": "M3x10", "category_id": category, "safety_stock": "20" })).await;
    let a = product(
        json!({ "code": "A001", "name": "螺帽 M3", "is_active": false, "safety_stock": "1" }),
    )
    .await;
    let c = product(json!({ "code": "C003", "name": "墊片" })).await;
    let service =
        product(json!({ "code": "S001", "name": "安裝費", "product_type": "service" })).await;
    let deleted = product(json!({ "code": "D004", "name": "已刪除" })).await;
    let (status, body) = call(
        pool,
        Method::DELETE,
        &format!("/api/v1/entities/product/records/{deleted}?version=1"),
        Some(&editor),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let gain = |p: Id, lot: &str, qty: &str, cost: &str| json!({ "data": { "product_id": p, "lot_no": lot, "qty": qty, "unit_cost_twd": cost } });
    let opening = posted(
        pool,
        &editor,
        "2026-11-01",
        "opening",
        json!([
            gain(b, "B-OLD", "10", "12.5"),
            gain(c, "C-01", "2.5", "100.1234")
        ]),
    )
    .await;
    let later = posted(
        pool,
        &editor,
        "2026-11-05",
        "count",
        json!([gain(b, "B-NEW", "4", "10.0005")]),
    )
    .await;
    let loss = posted(
        pool,
        &editor,
        "2026-11-06",
        "count",
        json!([{ "data": { "product_id": b, "line_type": "loss", "lot_no": "B-OLD", "qty": "10" } }]),
    )
    .await;
    let voided = posted(
        pool,
        &editor,
        "2026-11-02",
        "count",
        json!([gain(c, "C-X", "1", "1")]),
    )
    .await;
    let voided = act(pool, &editor, &voided, "void").await;
    Fixture {
        editor,
        b,
        a,
        c,
        service,
        deleted,
        category,
        opening,
        later,
        loss,
        voided,
    }
}

fn codes(page: &Page<StockBalance>) -> Vec<&str> {
    page.items.iter().map(|i| i.code.as_str()).collect()
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn balances_list_goods_with_filters(pool: PgPool) {
    let f = fixture(&pool).await;
    let viewer = token_for(&pool, "viewer", role::VIEWER).await;
    let balances = |query: &str| {
        let pool = pool.clone();
        let viewer = viewer.clone();
        let uri = format!("/api/v1/inventory/balances{query}");
        async move { get_page::<StockBalance>(&pool, &viewer, &uri).await }
    };

    // 範圍：未刪除的 goods（含停用）；排序 lower(code), id
    let all = balances("").await;
    assert_eq!(codes(&all), ["A001", "b-002", "C003"]);
    assert_eq!((all.total, all.page, all.page_size), (3, 1, 50));
    let json = serde_json::to_value(&all.items[1]).unwrap();
    assert_eq!(
        json,
        json!({
            "product_id": f.b, "code": "b-002", "name": "螺絲 M3", "spec": "M3x10", "unit_name": "個",
            "is_active": true, "qty_on_hand": "4.0000", "stock_value_twd": "40.00",
            "safety_stock": "20.0000", "below_safety_stock": true,
        })
    );
    let a = &all.items[0];
    assert_eq!(a.product_id, f.a);
    assert_eq!(
        (a.qty_on_hand.to_string(), a.stock_value_twd.to_string()),
        ("0.0000".into(), "0.00".into())
    );
    assert!(a.below_safety_stock);
    let c = &all.items[2];
    assert_eq!(c.product_id, f.c);
    // 2.5 × 100.1234 = 250.3085 → 250.31；作廢的盤盈不計
    assert_eq!(
        (c.qty_on_hand.to_string(), c.stock_value_twd.to_string()),
        ("2.5000".into(), "250.31".into())
    );
    assert_eq!((c.safety_stock, c.below_safety_stock), (None, false));
    assert!(
        !all.items
            .iter()
            .any(|i| i.product_id == f.service || i.product_id == f.deleted)
    );

    assert_eq!(codes(&balances("?q=m3").await), ["A001", "b-002"]);
    assert_eq!(
        codes(&balances("?q=%E8%9E%BA%E7%B5%B2%20b-0").await),
        ["b-002"]
    );
    assert_eq!(
        codes(&balances("?q=m3%20%E5%A2%8A").await),
        Vec::<&str>::new()
    );
    assert_eq!(
        codes(&balances(&format!("?category_id={}", f.category)).await),
        ["b-002"]
    );
    assert_eq!(codes(&balances("?in_stock=true").await), ["b-002", "C003"]);
    assert_eq!(codes(&balances("?in_stock=false").await), ["A001"]);
    assert_eq!(
        codes(&balances("?below_safety_stock=true").await),
        ["A001", "b-002"]
    );
    assert_eq!(
        codes(&balances("?include_inactive=false").await),
        ["b-002", "C003"]
    );
    assert_eq!(codes(&balances("?q=&in_stock=").await).len(), 3);

    let page2 = balances("?page=2&page_size=2").await;
    assert_eq!(codes(&page2), ["C003"]);
    assert_eq!((page2.total, page2.page, page2.page_size), (3, 2, 2));

    for (query, field) in [
        ("?in_stock=yes", "in_stock"),
        ("?category_id=abc", "category_id"),
        ("?include_inactive=1", "include_inactive"),
        ("?page=0", "page"),
    ] {
        let (status, body) = call(
            &pool,
            Method::GET,
            &format!("/api/v1/inventory/balances{query}"),
            Some(&f.editor),
            None,
        )
        .await;
        assert_invalid_param(status, &body, field);
    }
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn lots_of_product_in_fifo_order(pool: PgPool) {
    let f = fixture(&pool).await;
    let viewer = token_for(&pool, "viewer", role::VIEWER).await;
    let lots = |product: Id, query: &str| {
        let pool = pool.clone();
        let viewer = viewer.clone();
        let uri = format!("/api/v1/inventory/products/{product}/lots{query}");
        async move { get_page::<InventoryLot>(&pool, &viewer, &uri).await }
    };
    let nos = |page: &Page<InventoryLot>| -> Vec<String> {
        page.items.iter().map(|l| l.lot_no.clone()).collect()
    };

    // 預設只列出有庫存的批號
    let page = lots(f.b, "").await;
    assert_eq!(nos(&page), ["B-NEW"]);
    assert_eq!(page.total, 1);
    let json = serde_json::to_value(&page.items[0]).unwrap();
    assert_eq!(json["received_date"], json!("2026-11-05"));
    assert_eq!(json["source_type"], json!(LINE));
    assert_eq!(json["source_doc_id"], json!(f.later.id));
    assert_eq!(json["source_record_no"], json!(f.later.record_no));
    assert_eq!(json["qty_in"], json!("4.0000"));
    assert_eq!(json["qty_remaining"], json!("4.0000"));
    assert_eq!(json["unit_cost_twd"], json!("10.0005"));

    // 含已用完的批號，依先進先出順序
    let page = lots(f.b, "?include_empty=true").await;
    assert_eq!(nos(&page), ["B-OLD", "B-NEW"]);
    assert_eq!(page.items[0].qty_remaining.to_string(), "0.0000");
    assert_eq!(
        page.items[0].source_record_no,
        f.opening.record_no.clone().unwrap()
    );
    assert_eq!(
        nos(&lots(f.b, "?include_empty=true&q=old").await),
        ["B-OLD"]
    );
    let page = lots(f.b, "?include_empty=true&page=2&page_size=1").await;
    assert_eq!((nos(&page), page.total), (vec!["B-NEW".to_owned()], 2));

    // 作廢刪除的批號不出現
    assert_eq!(nos(&lots(f.c, "?include_empty=true").await), ["C-01"]);
    // 沒有批號的商品、服務類商品：空列表
    assert_eq!(lots(f.a, "").await.total, 0);
    let page = lots(f.service, "").await;
    assert_eq!((page.items.len(), page.total), (0, 0));

    // 商品不存在或已刪除 → 404
    for id in [f.deleted, hammer_shared::new_id()] {
        let (status, body) = call(
            &pool,
            Method::GET,
            &format!("/api/v1/inventory/products/{id}/lots"),
            Some(&viewer),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    }
    let (status, body) = call(
        &pool,
        Method::GET,
        &format!("/api/v1/inventory/products/{}/lots?include_empty=x", f.b),
        Some(&viewer),
        None,
    )
    .await;
    assert_invalid_param(status, &body, "include_empty");
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn transactions_newest_first_with_filters(pool: PgPool) {
    let f = fixture(&pool).await;
    let viewer = token_for(&pool, "viewer", role::VIEWER).await;
    let txns = |query: String| {
        let pool = pool.clone();
        let viewer = viewer.clone();
        let uri = format!("/api/v1/inventory/transactions{query}");
        async move { get_page::<InventoryTransaction>(&pool, &viewer, &uri).await }
    };
    let summary = |page: &Page<InventoryTransaction>| -> Vec<(String, String, String)> {
        page.items
            .iter()
            .map(|t| {
                (
                    t.txn_date.to_string(),
                    t.lot_no.clone(),
                    t.qty_change.to_string(),
                )
            })
            .collect()
    };
    let today = today_taipei().to_string();
    let s = |date: &str, lot: &str, qty: &str| (date.to_owned(), lot.to_owned(), qty.to_owned());

    // txn_date DESC, id DESC
    let all = txns(String::new()).await;
    assert_eq!(all.total, 6);
    assert_eq!(
        summary(&all),
        [
            s(&today, "C-X", "-1.0000"),
            s("2026-11-06", "B-OLD", "-10.0000"),
            s("2026-11-05", "B-NEW", "4.0000"),
            s("2026-11-02", "C-X", "1.0000"),
            s("2026-11-01", "C-01", "2.5000"),
            s("2026-11-01", "B-OLD", "10.0000"),
        ]
    );
    let json = serde_json::to_value(&all.items[1]).unwrap();
    assert_eq!(json["txn_type"], json!("adjustment"));
    assert_eq!(json["product_id"], json!(f.b));
    assert_eq!(json["product_code"], json!("b-002"));
    assert_eq!(json["product_name"], json!("螺絲 M3"));
    assert_eq!(json["unit_cost_twd"], json!("12.5000"));
    assert_eq!(json["ref_doc_type"], json!(DOC));
    assert_eq!(json["ref_doc_id"], json!(f.loss.id));
    assert_eq!(json["ref_record_no"], json!(f.loss.record_no));
    assert_eq!(
        json["ref_line_id"],
        json!(f.loss.lines.as_ref().unwrap()[LINE][0].id)
    );
    assert_eq!(json["created_by_name"], json!("editor"));
    assert!(json["created_by"].is_string());
    assert!(json["txn_at"].is_string());
    assert_eq!(all.items[0].ref_doc_id, f.voided.id);

    assert_eq!(txns(format!("?product_id={}", f.b)).await.total, 3);
    let b_old = all.items[1].lot_id;
    assert_eq!(
        summary(&txns(format!("?lot_id={b_old}")).await),
        [
            s("2026-11-06", "B-OLD", "-10.0000"),
            s("2026-11-01", "B-OLD", "10.0000")
        ]
    );
    assert_eq!(
        summary(&txns("?date_from=2026-11-02&date_to=2026-11-05".into()).await),
        [
            s("2026-11-05", "B-NEW", "4.0000"),
            s("2026-11-02", "C-X", "1.0000")
        ]
    );
    assert_eq!(
        summary(&txns("?txn_type=void_reversal".into()).await),
        [s(&today, "C-X", "-1.0000")]
    );
    let by_doc = txns(format!("?ref_doc_type={DOC}&ref_doc_id={}", f.opening.id)).await;
    assert_eq!(by_doc.total, 2);
    assert_eq!(txns("?ref_doc_type=shipment".into()).await.total, 0);
    let page = txns("?page=3&page_size=2".into()).await;
    assert_eq!((summary(&page).len(), page.total, page.page), (2, 6, 3));

    for (query, field) in [
        ("?product_id=abc", "product_id"),
        ("?lot_id=abc", "lot_id"),
        ("?ref_doc_id=abc", "ref_doc_id"),
        ("?date_from=2026-13-01", "date_from"),
        ("?date_to=yesterday", "date_to"),
        ("?date_from=2026-11-02&date_to=2026-11-01", "date_from"),
        ("?txn_type=transfer", "txn_type"),
    ] {
        let (status, body) = call(
            &pool,
            Method::GET,
            &format!("/api/v1/inventory/transactions{query}"),
            Some(&viewer),
            None,
        )
        .await;
        assert_invalid_param(status, &body, field);
    }
}

/// 未登入 `401`；頁碼不是整數 `400`。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn requires_login(pool: PgPool) {
    let viewer = token_for(&pool, "viewer", role::VIEWER).await;
    let product = hammer_shared::new_id();
    for uri in [
        "/api/v1/inventory/balances".to_owned(),
        format!("/api/v1/inventory/products/{product}/lots"),
        "/api/v1/inventory/transactions".to_owned(),
    ] {
        let (status, body) = call(&pool, Method::GET, &uri, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri} {body}");
        let (status, body) = call(
            &pool,
            Method::GET,
            &format!("{uri}?page=x"),
            Some(&viewer),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri} {body}");
    }
}

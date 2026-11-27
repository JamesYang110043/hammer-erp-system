//! 批次取得明細品名 API 的整合測試（#277，docs/architecture.md §8.9「批次取得明細品名 API」）：
//! 客戶品名與商品名稱的帶入、範圍（停用、已刪除、重複、其他客戶）、參數錯誤與權限。
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

async fn create(pool: &PgPool, token: &str, entity: &str, body: Value) -> Record {
    let (status, body) = call(pool, Method::POST, &records_uri(entity), token, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

const URI: &str = "/api/v1/sales/product-names";

async fn names(pool: &PgPool, token: &str, query: &str) -> (StatusCode, Value) {
    call(pool, Method::GET, &format!("{URI}?{query}"), token, None).await
}

fn query(customer: impl std::fmt::Display, products: &[Id]) -> String {
    let mut q = format!("customer_id={customer}");
    for p in products {
        q.push_str(&format!("&product_id={p}"));
    }
    q
}

/// `(product_id, customer_product_name, product_name)` 依回應順序。
fn rows(body: &Value) -> Vec<(Id, Option<String>, String)> {
    body.as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                serde_json::from_value(r["product_id"].clone()).unwrap(),
                r["customer_product_name"].as_str().map(str::to_owned),
                r["product_name"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn row(id: Id, customer_name: Option<&str>, name: &str) -> (Id, Option<String>, String) {
    (id, customer_name.map(str::to_owned), name.to_owned())
}

struct Master {
    a: Id,
    b: Id,
    /// P001 扳手 10mm、P002 鉗子、P003 停用的扳手、P004 已刪除的扳手、P005 套筒
    p: Vec<Id>,
}

/// 客戶 A：P001 → X 型扳手、P003 → 停用品名、P004 → 刪除品名、P005 → 已刪除的客戶品名（之後移除）；
/// 客戶 B：P002 → B 的鉗子。
async fn master_data(pool: &PgPool, token: &str) -> Master {
    let unit = create(
        pool,
        token,
        "unit",
        json!({ "data": { "code": "pcs", "name": "個" } }),
    )
    .await
    .id;
    let mut p = Vec::new();
    for (code, name) in [
        ("P001", "扳手 10mm"),
        ("P002", "鉗子"),
        ("P003", "停用的扳手"),
        ("P004", "已刪除的扳手"),
        ("P005", "套筒"),
    ] {
        let data = json!({ "code": code, "name": name, "unit_id": unit });
        p.push(
            create(pool, token, "product", json!({ "data": data }))
                .await
                .id,
        );
    }
    let a = create(
        pool,
        token,
        "customer",
        json!({
            "data": { "code": "C001", "name": "客戶 A" },
            "lines": { "customer_product_name": [
                { "data": { "product_id": p[0], "name": "X 型扳手" } },
                { "data": { "product_id": p[2], "name": "停用品名" } },
                { "data": { "product_id": p[3], "name": "刪除品名" } },
                { "data": { "product_id": p[4], "name": "舊的套筒品名" } },
            ] },
        }),
    )
    .await;
    // 客戶品名 P005 被刪除（整組取代時沒列出）
    let lines = a.lines.as_ref().unwrap()["customer_product_name"].clone();
    let keep: Vec<Value> = lines
        .iter()
        .take(3)
        .map(|l| json!({ "id": l.id, "data": {} }))
        .collect();
    let (status, body) = call(
        pool,
        Method::PATCH,
        &format!("{}/{}", records_uri("customer"), a.id),
        token,
        Some(json!({ "version": a.version, "data": {},
                     "lines": { "customer_product_name": keep } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let b = create(
        pool,
        token,
        "customer",
        json!({
            "data": { "code": "C002", "name": "客戶 B" },
            "lines": { "customer_product_name": [
                { "data": { "product_id": p[1], "name": "B 的鉗子" } },
            ] },
        }),
    )
    .await
    .id;
    sqlx::query("UPDATE products SET is_active = false WHERE id = $1")
        .bind(p[2])
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("UPDATE products SET deleted_at = now() WHERE id = $1")
        .bind(p[3])
        .execute(pool)
        .await
        .unwrap();
    Master { a: a.id, b, p }
}

/// 客戶品名優先、沒有時為商品名稱；停用的商品回傳，已刪除、不存在的略過，重複只回傳一筆；
/// 依第一次出現的順序；只用這位客戶未刪除的客戶品名。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn resolves_names_in_request_order(pool: PgPool) {
    let token = user(&pool, "viewer", role::VIEWER).await;
    let editor = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &editor).await;
    let p = &m.p;

    let ids = [p[4], p[0], p[1], p[0], p[2], p[3], Id::now_v7()];
    let (status, body) = names(&pool, &token, &query(m.a, &ids)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        rows(&body),
        [
            row(p[4], None, "套筒"),
            row(p[0], Some("X 型扳手"), "X 型扳手"),
            row(p[1], None, "鉗子"),
            row(p[2], Some("停用品名"), "停用品名"),
        ]
    );

    // 客戶 B 只用自己的客戶品名
    let (status, body) = names(&pool, &token, &query(m.b, &[p[0], p[1]])).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        rows(&body),
        [
            row(p[0], None, "扳手 10mm"),
            row(p[1], Some("B 的鉗子"), "B 的鉗子")
        ]
    );

    // 停用的客戶可以
    sqlx::query("UPDATE customers SET is_active = false WHERE id = $1")
        .bind(m.b)
        .execute(&pool)
        .await
        .unwrap();
    let (status, body) = names(&pool, &token, &query(m.b, &[p[1]])).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(rows(&body), [row(p[1], Some("B 的鉗子"), "B 的鉗子")]);
}

/// 參數錯誤、客戶不存在或已刪除、沒有權限。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn errors(pool: PgPool) {
    let token = user(&pool, "viewer", role::VIEWER).await;
    let editor = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &editor).await;
    let p0 = m.p[0];

    for (q, field) in [
        (format!("product_id={p0}"), "customer_id"),
        (format!("customer_id=&product_id={p0}"), "customer_id"),
        (format!("customer_id=abc&product_id={p0}"), "customer_id"),
        (format!("customer_id={}", m.a), "product_id"),
        (format!("customer_id={}&product_id=abc", m.a), "product_id"),
        (query(m.a, &vec![p0; 201]), "product_id"),
    ] {
        let (status, body) = names(&pool, &token, &q).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{q}: {body}");
        assert_eq!(body["details"]["field"], field, "{q}");
    }
    let (status, body) = names(&pool, &token, &query(m.a, &vec![p0; 200])).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(rows(&body).len(), 1);

    let (status, body) = names(&pool, &token, &query(Id::now_v7(), &[p0])).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["message"], "找不到此客戶");
    sqlx::query("UPDATE customers SET deleted_at = now() WHERE id = $1")
        .bind(m.b)
        .execute(&pool)
        .await
        .unwrap();
    let (status, body) = names(&pool, &token, &query(m.b, &[p0])).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // 沒有 record.read → 403
    sqlx::query(
        "DELETE FROM role_permissions
         WHERE permission = 'record.read'
           AND role_id = (SELECT id FROM roles WHERE code = 'viewer')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let (status, body) = names(&pool, &token, &query(m.a, &[p0])).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

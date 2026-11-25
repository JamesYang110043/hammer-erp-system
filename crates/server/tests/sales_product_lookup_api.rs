//! 銷貨用商品選擇 API 的整合測試（#266，docs/architecture.md §8.9「銷貨用商品選擇 API」）：
//! 以客戶品名搜尋、排序、範圍（停用、已刪除、其他客戶）、參數錯誤與權限。
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

const URI: &str = "/api/v1/sales/product-lookup";

async fn lookup(pool: &PgPool, token: &str, query: &str) -> (StatusCode, Value) {
    call(pool, Method::GET, &format!("{URI}?{query}"), token, None).await
}

/// `(code, customer_product_name, product_name)` 依回應順序。
fn rows(body: &Value) -> Vec<(String, Option<String>, String)> {
    body.as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["code"].as_str().unwrap().to_owned(),
                r["customer_product_name"].as_str().map(str::to_owned),
                r["product_name"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn row(code: &str, customer_name: Option<&str>, name: &str) -> (String, Option<String>, String) {
    (
        code.to_owned(),
        customer_name.map(str::to_owned),
        name.to_owned(),
    )
}

struct Master {
    a: Id,
    b: Id,
}

/// 商品 P001 扳手 10mm、P002 鉗子、P003 扳手組、P004 停用的扳手、P005 已刪除的扳手（依建立順序）；
/// 客戶 A：P001 → X 型扳手、P002 → 工具組零件 03、P004 → 停用品名、P005 → 刪除品名；客戶 B 沒有設定。
async fn master_data(pool: &PgPool, token: &str) -> Master {
    let unit = create(
        pool,
        token,
        "unit",
        json!({ "data": { "code": "pcs", "name": "個" } }),
    )
    .await
    .id;
    let mut ids = Vec::new();
    for (code, name, spec, price) in [
        ("P001", "扳手 10mm", Some("鍍鉻"), Some("120")),
        ("P002", "鉗子", None, None),
        ("P003", "扳手組", None, None),
        ("P004", "停用的扳手", None, None),
        ("P005", "已刪除的扳手", None, None),
    ] {
        let mut data = json!({ "code": code, "name": name, "unit_id": unit });
        if let Some(spec) = spec {
            data["spec"] = json!(spec);
        }
        if let Some(price) = price {
            data["default_sale_price"] = json!(price);
        }
        ids.push(
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
                { "data": { "product_id": ids[0], "name": "X 型扳手" } },
                { "data": { "product_id": ids[1], "name": "工具組零件 03" } },
                { "data": { "product_id": ids[3], "name": "停用品名" } },
                { "data": { "product_id": ids[4], "name": "刪除品名" } },
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
    sqlx::query("UPDATE products SET is_active = false WHERE id = $1")
        .bind(ids[3])
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("UPDATE products SET deleted_at = now() WHERE id = $1")
        .bind(ids[4])
        .execute(pool)
        .await
        .unwrap();
    Master { a, b }
}

/// 以客戶品名的一部分找到商品；客戶品名符合的排在前面；料號、商品名稱仍可找到。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn search_by_customer_product_name(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;

    // 「零件」只在客戶品名中
    let (status, body) = lookup(&pool, &token, &format!("customer_id={}&q=零件", m.a)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        rows(&body),
        [row("P002", Some("工具組零件 03"), "工具組零件 03")]
    );

    // 「扳手」：客戶品名符合的 P001 在前，其他依由新到舊（P003 在 P001 之後建立）；停用、已刪除的不出現
    let (_, body) = lookup(&pool, &token, &format!("customer_id={}&q=扳手", m.a)).await;
    assert_eq!(
        rows(&body),
        [
            row("P001", Some("X 型扳手"), "X 型扳手"),
            row("P003", None, "扳手組"),
        ]
    );
    let first = &body[0];
    assert_eq!(first["title"], "P001 扳手 10mm");
    assert_eq!(first["name"], "扳手 10mm");
    assert_eq!(first["spec"], "鍍鉻");
    assert_eq!(first["product_type"], "goods");
    assert_eq!(first["default_sale_price"], "120.0000");
    assert_eq!(body[1]["spec"], Value::Null);
    assert_eq!(body[1]["default_sale_price"], Value::Null);

    // 料號、商品名稱（客戶品名不含「鉗」）
    let (_, body) = lookup(&pool, &token, &format!("customer_id={}&q=p002", m.a)).await;
    assert_eq!(
        rows(&body),
        [row("P002", Some("工具組零件 03"), "工具組零件 03")]
    );
    let (_, body) = lookup(&pool, &token, &format!("customer_id={}&q=鉗子", m.a)).await;
    assert_eq!(
        rows(&body),
        [row("P002", Some("工具組零件 03"), "工具組零件 03")]
    );

    // 料號與客戶品名混用
    let (_, body) = lookup(&pool, &token, &format!("customer_id={}&q=P001+X+型", m.a)).await;
    assert_eq!(rows(&body), [row("P001", Some("X 型扳手"), "X 型扳手")]);

    // 沒有關鍵字：有客戶品名的在前（各自由新到舊）
    let (_, body) = lookup(&pool, &token, &format!("customer_id={}", m.a)).await;
    let codes: Vec<String> = rows(&body).into_iter().map(|r| r.0).collect();
    assert_eq!(codes, ["P002", "P001", "P003"]);

    // limit
    let (_, body) = lookup(&pool, &token, &format!("customer_id={}&limit=1", m.a)).await;
    assert_eq!(rows(&body).len(), 1);
}

/// 其他客戶的客戶品名不會被比對到；沒帶 customer_id 時與一般商品搜尋相同。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn other_customers_and_no_customer(pool: PgPool) {
    let token = user(&pool, "viewer", role::VIEWER).await;
    let editor = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &editor).await;

    let (status, body) = lookup(&pool, &token, &format!("customer_id={}&q=零件", m.b)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(rows(&body), []);
    let (_, body) = lookup(&pool, &token, &format!("customer_id={}&q=扳手", m.b)).await;
    assert_eq!(
        rows(&body),
        [row("P003", None, "扳手組"), row("P001", None, "扳手 10mm")]
    );

    for query in ["q=扳手", "customer_id=&q=扳手"] {
        let (status, body) = lookup(&pool, &token, query).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            rows(&body),
            [row("P003", None, "扳手組"), row("P001", None, "扳手 10mm")],
            "{query}"
        );
    }
    let (_, body) = lookup(&pool, &token, "q=零件").await;
    assert_eq!(rows(&body), []);
}

/// 參數錯誤、客戶不存在或已刪除、沒有權限。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn errors(pool: PgPool) {
    let token = user(&pool, "viewer", role::VIEWER).await;
    let editor = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &editor).await;

    let (status, body) = lookup(&pool, &token, "customer_id=abc").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["details"]["field"], "customer_id");

    let (status, body) = lookup(&pool, &token, &format!("customer_id={}", Id::now_v7())).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    sqlx::query("UPDATE customers SET deleted_at = now() WHERE id = $1")
        .bind(m.b)
        .execute(&pool)
        .await
        .unwrap();
    let (status, body) = lookup(&pool, &token, &format!("customer_id={}", m.b)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    let (status, body) = lookup(&pool, &token, "limit=0").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, body) = lookup(&pool, &token, "limit=x").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    // 沒有 record.read → 403
    sqlx::query(
        "DELETE FROM role_permissions
         WHERE permission = 'record.read'
           AND role_id = (SELECT id FROM roles WHERE code = 'viewer')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let (status, body) = lookup(&pool, &token, &format!("customer_id={}", m.a)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

//! 商品被庫存引用時的限制（#133，docs/architecture.md §7.2「商品被庫存引用時的限制」）：有庫存異動的
//! 商品不可刪除、不可改為服務、不可修改單位；沒有異動的商品（含只被草稿引用）不受影響。
//!
//! 使用 `#[sqlx::test]`，需要 `DATABASE_URL`（見 README「整合測試」）。

#![allow(clippy::unwrap_used)]

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use hammer_core::test_util::{TestUser, create_user_with_session};
use hammer_server::{AppState, Config, router};
use hammer_shared::record::Record;
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

async fn editor(pool: &PgPool) -> String {
    let (_, token) = create_user_with_session(pool, &TestUser::new("editor", role::EDITOR))
        .await
        .unwrap();
    token.into_string()
}

fn uri(entity: &str, id: Option<Id>) -> String {
    match id {
        Some(id) => format!("/api/v1/entities/{entity}/records/{id}"),
        None => format!("/api/v1/entities/{entity}/records"),
    }
}

async fn create(pool: &PgPool, token: &str, entity: &str, data: Value, lines: Value) -> Record {
    let (status, body) = call(
        pool,
        Method::POST,
        &uri(entity, None),
        token,
        Some(json!({ "data": data, "lines": lines })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn get(pool: &PgPool, token: &str, entity: &str, id: Id) -> Record {
    let (status, body) = call(pool, Method::GET, &uri(entity, Some(id)), token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn act(pool: &PgPool, token: &str, record: &Record, action: &str) -> Record {
    let (status, body) = call(
        pool,
        Method::POST,
        &format!("{}/{action}", uri(DOC, Some(record.id))),
        token,
        Some(json!({ "version": record.version })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn patch_product(pool: &PgPool, token: &str, id: Id, data: Value) -> (StatusCode, Value) {
    let version = get(pool, token, "product", id).await.version;
    call(
        pool,
        Method::PATCH,
        &uri("product", Some(id)),
        token,
        Some(json!({ "version": version, "data": data })),
    )
    .await
}

async fn delete_product(pool: &PgPool, token: &str, id: Id) -> (StatusCode, Value) {
    let version = get(pool, token, "product", id).await.version;
    call(
        pool,
        Method::DELETE,
        &format!("{}?version={version}", uri("product", Some(id))),
        token,
        None,
    )
    .await
}

struct Fixture {
    token: String,
    pcs: Id,
    box_unit: Id,
}

async fn fixture(pool: &PgPool) -> Fixture {
    let token = editor(pool).await;
    let unit = |code: &'static str, name: &'static str| {
        let token = token.clone();
        async move {
            create(
                pool,
                &token,
                "unit",
                json!({ "code": code, "name": name }),
                json!({}),
            )
            .await
            .id
        }
    };
    let pcs = unit("pcs", "個").await;
    let box_unit = unit("box", "盒").await;
    Fixture {
        token,
        pcs,
        box_unit,
    }
}

async fn product(pool: &PgPool, f: &Fixture, code: &str, product_type: &str) -> Id {
    create(
        pool,
        &f.token,
        "product",
        json!({ "code": code, "name": code, "unit_id": f.pcs, "product_type": product_type }),
        json!({}),
    )
    .await
    .id
}

/// 盤盈新批號一列的盤點調整單草稿。
async fn draft(pool: &PgPool, f: &Fixture, product: Id) -> Record {
    create(
        pool,
        &f.token,
        DOC,
        json!({ "adjustment_date": "2026-11-01" }),
        json!({ LINE: [{ "data": { "product_id": product, "qty": "3", "unit_cost_twd": "5" } }] }),
    )
    .await
}

fn assert_field_errors(status: StatusCode, body: &Value, expected: &[(&str, &str)]) {
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let err: ApiError = serde_json::from_value(body.clone()).unwrap();
    assert_eq!(err.code, ErrorCode::Validation, "{body}");
    let actual: Vec<(String, String)> = err.details.unwrap()["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["field_key"].as_str().unwrap().to_owned(),
                f["message"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let expected: Vec<(String, String)> = expected
        .iter()
        .map(|(k, m)| ((*k).to_owned(), (*m).to_owned()))
        .collect();
    assert_eq!(actual, expected, "{body}");
}

/// 有庫存異動的商品：不可刪除、改為服務、修改單位；其他欄位可以修改。作廢後異動仍在，限制不變。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn product_with_stock_transactions_is_restricted(pool: PgPool) {
    let f = fixture(&pool).await;
    let p = product(&pool, &f, "A001", "goods").await;
    let posted = act(&pool, &f.token, &draft(&pool, &f, p).await, "post").await;

    let (status, body) = delete_product(&pool, &f.token, p).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let err: ApiError = serde_json::from_value(body.clone()).unwrap();
    assert_eq!(err.code, ErrorCode::Validation);
    assert_eq!(err.message, "此商品已有庫存異動，無法刪除，請改為停用");
    assert_eq!(
        err.details,
        Some(json!({ "fields": [], "transaction_count": 1 }))
    );

    let (status, body) =
        patch_product(&pool, &f.token, p, json!({ "product_type": "service" })).await;
    assert_field_errors(
        status,
        &body,
        &[("product_type", "此商品已有庫存異動，無法改為服務")],
    );
    let (status, body) = patch_product(&pool, &f.token, p, json!({ "unit_id": f.box_unit })).await;
    assert_field_errors(
        status,
        &body,
        &[("unit_id", "此商品已有庫存異動，無法修改單位")],
    );
    let (status, body) = patch_product(
        &pool,
        &f.token,
        p,
        json!({ "product_type": "service", "unit_id": f.box_unit }),
    )
    .await;
    assert_field_errors(
        status,
        &body,
        &[
            ("product_type", "此商品已有庫存異動，無法改為服務"),
            ("unit_id", "此商品已有庫存異動，無法修改單位"),
        ],
    );

    // 其他欄位（含停用）可以修改；值不變的 unit_id、product_type 不檢查
    let (status, body) = patch_product(
        &pool,
        &f.token,
        p,
        json!({ "name": "螺絲（舊）", "is_active": false, "unit_id": f.pcs, "product_type": "goods" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // 作廢後異動仍在：限制不變（異動 2 筆）
    act(&pool, &f.token, &posted, "void").await;
    let (status, body) = delete_product(&pool, &f.token, p).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["details"]["transaction_count"], json!(2));
    let product = get(&pool, &f.token, "product", p).await;
    assert_eq!(product.data["product_type"], json!("goods"));
    assert_eq!(product.data["unit_id"], json!(f.pcs));
}

/// 沒有庫存異動的商品（含只被草稿引用、服務類商品）不受影響。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn product_without_stock_transactions_is_not_restricted(pool: PgPool) {
    let f = fixture(&pool).await;

    // 只被草稿引用：可以改單位、改類型、刪除
    let in_draft = product(&pool, &f, "A001", "goods").await;
    draft(&pool, &f, in_draft).await;
    let (status, body) =
        patch_product(&pool, &f.token, in_draft, json!({ "unit_id": f.box_unit })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = patch_product(
        &pool,
        &f.token,
        in_draft,
        json!({ "product_type": "service" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = delete_product(&pool, &f.token, in_draft).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    // 服務改為實體商品可以
    let service = product(&pool, &f, "S001", "service").await;
    let (status, body) =
        patch_product(&pool, &f.token, service, json!({ "product_type": "goods" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // 其他商品有異動不影響這個商品
    let other = product(&pool, &f, "B001", "goods").await;
    act(&pool, &f.token, &draft(&pool, &f, other).await, "post").await;
    let plain = product(&pool, &f, "C001", "goods").await;
    let (status, body) =
        patch_product(&pool, &f.token, plain, json!({ "unit_id": f.box_unit })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = delete_product(&pool, &f.token, plain).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
}

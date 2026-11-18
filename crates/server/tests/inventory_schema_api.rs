//! M4a 盤點調整單的定義 API 整合測試（#128）：migration 寫入的系統單據 `stock_adjustment` 與明細子表
//! `stock_adjustment_line` 可以透過資料類別 API 讀取，欄位定義與 docs/architecture.md §8.10 一致。
//!
//! 使用 `#[sqlx::test]`，需要 `DATABASE_URL`（見 README「整合測試」）。

#![allow(clippy::unwrap_used)]

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use hammer_core::test_util::{TestUser, create_user_with_session};
use hammer_server::{AppState, Config, router};
use hammer_shared::role;
use hammer_shared::schema::{
    DataType, EntityDefinition, EntityDetail, EntityKind, EntityStorage, FieldStorage,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

fn app(pool: &PgPool) -> Router {
    let config = Config::from_sources(None, |name| match name {
        "DATABASE_URL" => Some("postgres://unused".to_owned()),
        _ => None,
    })
    .unwrap();
    router(AppState::new(pool.clone(), config))
}

async fn get(pool: &PgPool, uri: &str, token: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::USER_AGENT, "HammerERP-test")
        .header("X-Client-Version", "0.1.0")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let res = app(pool).oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn viewer_token(pool: &PgPool) -> String {
    let (_, token) = create_user_with_session(pool, &TestUser::new("viewer", role::VIEWER))
        .await
        .unwrap();
    token.into_string()
}

async fn get_entity(pool: &PgPool, token: &str, key: &str) -> EntityDetail {
    let (status, body) = get(pool, &format!("/api/v1/entities/{key}"), token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

/// 選項值。
fn option_values(detail: &EntityDetail, key: &str) -> Vec<String> {
    let field = detail.fields.iter().find(|f| f.field_key == key).unwrap();
    field
        .options
        .as_ref()
        .unwrap()
        .iter()
        .map(|o| o.value.clone())
        .collect()
}

/// 列表出現 `stock_adjustment`（document、預設單號規則）與其明細子表。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn list_includes_stock_adjustment(pool: PgPool) {
    let token = viewer_token(&pool).await;
    let (status, body) = get(&pool, "/api/v1/entities", &token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let list: Vec<EntityDefinition> = serde_json::from_value(body).unwrap();

    let doc = list.iter().find(|e| e.key == "stock_adjustment").unwrap();
    assert_eq!(doc.label, "盤點調整單");
    assert_eq!(doc.kind, EntityKind::Document);
    assert!(doc.is_system && doc.is_active);
    assert_eq!(doc.storage, EntityStorage::Table);
    assert_eq!(doc.parent_entity_id, None);
    assert_eq!(doc.numbering_pattern.as_deref(), Some("ADJ{yyyyMM}-{0000}"));
    assert_eq!(doc.title_template.as_deref(), Some("{record_no}"));
    assert_eq!(doc.icon, None);
    assert_eq!(doc.sort_order, 100);

    let line = list
        .iter()
        .find(|e| e.key == "stock_adjustment_line")
        .unwrap();
    assert_eq!(line.label, "盤點調整明細");
    assert_eq!(line.kind, EntityKind::Detail);
    assert!(line.is_system && line.is_active);
    assert_eq!(line.parent_entity_id, Some(doc.id));
    assert_eq!(line.numbering_pattern, None);
    assert_eq!(
        line.title_template.as_deref(),
        Some("{product_id.code} {product_id.name} {lot_no}")
    );
    assert_eq!(line.sort_order, 0);
}

/// 主表的系統欄位：`record_no`、`posted_at` 等不是欄位；`status` 為唯讀、不必填、沒有預設值。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn stock_adjustment_fields(pool: PgPool) {
    let token = viewer_token(&pool).await;
    let detail = get_entity(&pool, &token, "stock_adjustment").await;

    let keys: Vec<&str> = detail.fields.iter().map(|f| f.field_key.as_str()).collect();
    assert_eq!(
        keys,
        ["adjustment_date", "adjustment_type", "status", "notes"]
    );
    for f in &detail.fields {
        assert!(
            f.is_system && f.is_active && f.is_visible,
            "{}",
            f.field_key
        );
        assert_eq!(f.storage, FieldStorage::Column, "{}", f.field_key);
    }

    let date = &detail.fields[0];
    assert_eq!(
        (date.label.as_str(), date.data_type, date.is_required),
        ("調整日期", DataType::Date, true)
    );

    let kind = &detail.fields[1];
    assert_eq!(
        (kind.label.as_str(), kind.data_type, kind.is_required),
        ("類型", DataType::Select, true)
    );
    assert_eq!(kind.default_value, Some(json!("count")));
    assert_eq!(
        option_values(&detail, "adjustment_type"),
        ["opening", "count", "adjustment"]
    );

    let status = &detail.fields[2];
    assert_eq!(
        (status.label.as_str(), status.data_type),
        ("狀態", DataType::Select)
    );
    assert!(status.is_readonly && !status.is_required);
    assert_eq!(status.default_value, None);
    assert_eq!(
        option_values(&detail, "status"),
        ["draft", "posted", "void"]
    );

    let notes = &detail.fields[3];
    assert_eq!(
        (notes.label.as_str(), notes.data_type, notes.is_required),
        ("備註", DataType::LongText, false)
    );
}

/// 明細的系統欄位：父資料欄位、`sort_order`、`lot_id` 不是欄位；`product_id` 指向商品。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn stock_adjustment_line_fields(pool: PgPool) {
    let token = viewer_token(&pool).await;
    let detail = get_entity(&pool, &token, "stock_adjustment_line").await;
    let product = get_entity(&pool, &token, "product").await;

    let keys: Vec<&str> = detail.fields.iter().map(|f| f.field_key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "product_id",
            "line_type",
            "lot_no",
            "qty",
            "unit_cost_twd",
            "reason"
        ]
    );
    for f in &detail.fields {
        assert!(
            f.is_system && f.is_active && !f.is_readonly,
            "{}",
            f.field_key
        );
        assert_eq!(f.storage, FieldStorage::Column, "{}", f.field_key);
    }
    let f = |key: &str| detail.fields.iter().find(|f| f.field_key == key).unwrap();

    let product_id = f("product_id");
    assert_eq!(product_id.data_type, DataType::Relation);
    assert!(product_id.is_required);
    assert_eq!(product_id.relation_entity_id, Some(product.entity.id));

    let line_type = f("line_type");
    assert_eq!(line_type.data_type, DataType::Select);
    assert!(line_type.is_required);
    assert_eq!(line_type.default_value, Some(json!("gain_new")));
    assert_eq!(
        option_values(&detail, "line_type"),
        ["gain_new", "gain_existing", "loss"]
    );

    let lot_no = f("lot_no");
    assert_eq!(lot_no.data_type, DataType::Text);
    assert!(!lot_no.is_required);
    assert_eq!(lot_no.validation.as_ref().unwrap().max_length, Some(50));

    let qty = f("qty");
    assert_eq!(qty.data_type, DataType::Decimal);
    assert!(qty.is_required);
    assert_eq!(qty.decimal_places, Some(4));
    assert_eq!(
        qty.validation.as_ref().unwrap().min.unwrap().to_string(),
        "0"
    );

    let cost = f("unit_cost_twd");
    assert_eq!(cost.data_type, DataType::Money);
    assert!(!cost.is_required);
    assert_eq!(cost.decimal_places, Some(4));
    assert_eq!(
        cost.validation.as_ref().unwrap().min.unwrap().to_string(),
        "0"
    );

    let reason = f("reason");
    assert_eq!(reason.data_type, DataType::Text);
    assert!(!reason.is_required);
}

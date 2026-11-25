//! M3 系統資料類別的定義 API 整合測試（#105）：migration 寫入的 7 個系統資料類別與系統欄位
//! 可以透過資料類別、欄位定義 API 讀取；不可刪除、停用，系統欄位不可停用、變更型態
//! （docs/architecture.md §7.1、§7.2、§8.6、§8.7）。
//!
//! 使用 `#[sqlx::test]`，需要 `DATABASE_URL`（見 README「整合測試」）。

#![allow(clippy::unwrap_used)]

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use hammer_core::test_util::{TestUser, create_user_with_session};
use hammer_server::{AppState, Config, router};
use hammer_shared::schema::{
    DataType, EntityDefinition, EntityDetail, EntityKind, FieldDefinition, FieldStorage,
};
use hammer_shared::{ApiError, ErrorCode, role};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

/// §8.6「M3 的系統資料類別」：代碼、種類、主表、系統欄位數。
const SYSTEM_ENTITIES: &[(&str, EntityKind, Option<&str>, usize)] = &[
    ("product", EntityKind::Master, None, 12),
    ("product_category", EntityKind::Master, None, 3),
    ("unit", EntityKind::Master, None, 2),
    ("customer", EntityKind::Master, None, 14),
    ("customer_contact", EntityKind::Detail, Some("customer"), 7),
    ("supplier", EntityKind::Master, None, 17),
    ("supplier_contact", EntityKind::Detail, Some("supplier"), 7),
];

fn app(pool: &PgPool) -> Router {
    let config = Config::from_sources(None, |name| match name {
        "DATABASE_URL" => Some("postgres://unused".to_owned()),
        _ => None,
    })
    .unwrap();
    router(AppState::new(pool.clone(), config))
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
    let res = app(pool).oneshot(req.body(body).unwrap()).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, value)
}

/// 422 VALIDATION，且 `details.field` 為 `field`。
fn assert_invalid_field(status: StatusCode, body: &Value, field: &str) {
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let err: ApiError = serde_json::from_value(body.clone()).unwrap();
    assert_eq!(err.code, ErrorCode::Validation, "{body}");
    assert_eq!(err.details.as_ref().unwrap()["field"], field, "{body}");
}

async fn token_for(pool: &PgPool, username: &str, role: &str) -> String {
    let (_, token) = create_user_with_session(pool, &TestUser::new(username, role))
        .await
        .unwrap();
    token.into_string()
}

async fn get_entity(pool: &PgPool, token: &str, key: &str) -> EntityDetail {
    let (status, body) = call(
        pool,
        Method::GET,
        &format!("/api/v1/entities/{key}"),
        token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

fn field<'a>(detail: &'a EntityDetail, key: &str) -> &'a FieldDefinition {
    detail.fields.iter().find(|f| f.field_key == key).unwrap()
}

fn field_patch(field: &FieldDefinition, changes: Value) -> Value {
    let mut body = json!({
        "version": field.version,
        "label": field.label,
        "data_type": field.data_type,
        "is_required": field.is_required,
        "is_visible": field.is_visible,
        "is_readonly": field.is_readonly,
        "is_searchable": field.is_searchable,
        "is_unique": field.is_unique,
        "default_value": field.default_value,
        "validation": field.validation,
        "options": field.options,
        "decimal_places": field.decimal_places,
        "ui_widget": field.ui_widget,
        "column_width": field.column_width,
        "help_text": field.help_text,
        "sort_order": field.sort_order,
        "is_active": field.is_active,
    });
    for (k, v) in changes.as_object().unwrap() {
        body[k] = v.clone();
    }
    body
}

async fn patch_field(
    pool: &PgPool,
    token: &str,
    entity: &str,
    field: &FieldDefinition,
    changes: Value,
) -> (StatusCode, Value) {
    call(
        pool,
        Method::PATCH,
        &format!("/api/v1/entities/{entity}/fields/{}", field.field_key),
        token,
        Some(field_patch(field, changes)),
    )
    .await
}

/// 登入後（含唯讀者）列表回傳 M3 的 7 個系統資料類別（與 M4a 的盤點調整單、M4b 的進貨單據、M4c 的銷貨單據、M5 的發票單據）；明細子表有主表，桌面程式的選單依 `kind` 排除。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn list_returns_system_entities(pool: PgPool) {
    let token = token_for(&pool, "viewer", role::VIEWER).await;
    let (status, body) = call(&pool, Method::GET, "/api/v1/entities", &token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let list: Vec<EntityDefinition> = serde_json::from_value(body).unwrap();

    let keys: Vec<&str> = list.iter().map(|e| e.key.as_str()).collect();
    // 依 sort_order、id 排序：明細子表（0）在前；客戶品名（M11，#264）的 sort_order 為 10，排在商品之後；盤點調整單（M4a，#128）、進貨單據（M4b，#138）、銷貨單據（M4c，#146）、發票單據（M5，#183）在 M3 之後
    assert_eq!(
        keys,
        [
            "customer_contact",
            "supplier_contact",
            "stock_adjustment_line",
            "purchase_order_line",
            "goods_receipt_line",
            "purchase_return_line",
            "sales_order_line",
            "shipment_line",
            "sales_return_line",
            "sales_invoice_line",
            "sales_invoice_shipment",
            "sales_allowance_line",
            "purchase_invoice_receipt",
            "product",
            "customer_product_name",
            "product_category",
            "unit",
            "customer",
            "supplier",
            "stock_adjustment",
            "purchase_order",
            "goods_receipt",
            "purchase_return",
            "sales_order",
            "shipment",
            "sales_return",
            "sales_invoice",
            "sales_allowance",
            "purchase_invoice",
            "purchase_allowance",
        ]
    );
    for (key, kind, parent, _) in SYSTEM_ENTITIES {
        let entity = list.iter().find(|e| e.key == *key).unwrap();
        assert!(entity.is_system && entity.is_active, "{key}");
        assert_eq!(entity.kind, *kind, "{key}");
        let parent_id = parent.map(|p| list.iter().find(|e| e.key == p).unwrap().id);
        assert_eq!(entity.parent_entity_id, parent_id, "{key}");
        assert_eq!(entity.numbering_pattern, None, "{key}");
    }
    let menu: Vec<&str> = list
        .iter()
        .filter(|e| e.kind != EntityKind::Detail)
        .map(|e| e.key.as_str())
        .collect();
    assert_eq!(
        menu,
        [
            "product",
            "product_category",
            "unit",
            "customer",
            "supplier",
            "stock_adjustment",
            "purchase_order",
            "goods_receipt",
            "purchase_return",
            "sales_order",
            "shipment",
            "sales_return",
            "sales_invoice",
            "sales_allowance",
            "purchase_invoice",
            "purchase_allowance",
        ]
    );
}

/// 單筆資料類別回傳系統欄位（`is_system`、`storage = column`）；關聯欄位指向系統資料類別。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn get_returns_system_fields(pool: PgPool) {
    let token = token_for(&pool, "viewer", role::VIEWER).await;
    for (key, _, _, count) in SYSTEM_ENTITIES {
        let detail = get_entity(&pool, &token, key).await;
        assert_eq!(detail.fields.len(), *count, "{key}");
        for f in &detail.fields {
            assert!(f.is_system && f.is_active, "{key}.{}", f.field_key);
            assert_eq!(f.storage, FieldStorage::Column, "{key}.{}", f.field_key);
        }
    }

    let product = get_entity(&pool, &token, "product").await;
    let unit = get_entity(&pool, &token, "unit").await;
    let category = get_entity(&pool, &token, "product_category").await;
    assert_eq!(
        product.entity.title_template.as_deref(),
        Some("{code} {name}")
    );

    let unit_id = field(&product, "unit_id");
    assert_eq!(unit_id.data_type, DataType::Relation);
    assert!(unit_id.is_required);
    assert_eq!(unit_id.relation_entity_id, Some(unit.entity.id));
    assert_eq!(
        field(&product, "category_id").relation_entity_id,
        Some(category.entity.id)
    );
    assert_eq!(
        field(&category, "parent_id").relation_entity_id,
        Some(category.entity.id)
    );

    let product_type = field(&product, "product_type");
    assert_eq!(product_type.data_type, DataType::Select);
    assert_eq!(product_type.default_value, Some(json!("goods")));
    let options: Vec<&str> = product_type
        .options
        .as_ref()
        .unwrap()
        .iter()
        .map(|o| o.value.as_str())
        .collect();
    assert_eq!(options, ["goods", "service"]);

    let code = field(&product, "code");
    assert!(code.is_required && code.is_unique);
    assert_eq!(code.validation.as_ref().unwrap().max_length, Some(50));

    let price = field(&product, "default_sale_price");
    assert_eq!(price.data_type, DataType::Money);
    assert_eq!(price.decimal_places, Some(4));
    assert_eq!(
        price.validation.as_ref().unwrap().min.unwrap().to_string(),
        "0"
    );

    let supplier = get_entity(&pool, &token, "supplier").await;
    let currency = field(&supplier, "default_currency_code");
    assert_eq!(currency.default_value, Some(json!("TWD")));
    assert_eq!(currency.ui_widget.as_deref(), Some("currency"));
}

/// 系統資料類別不可停用、不可刪除。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn system_entities_cannot_be_deactivated_or_deleted(pool: PgPool) {
    let token = token_for(&pool, "admin", role::ADMIN).await;
    for (key, _, _, _) in SYSTEM_ENTITIES {
        let entity = get_entity(&pool, &token, key).await.entity;
        let (status, body) = call(
            &pool,
            Method::PATCH,
            &format!("/api/v1/entities/{key}"),
            &token,
            Some(json!({
                "version": entity.version,
                "label": entity.label,
                "icon": entity.icon,
                "numbering_pattern": entity.numbering_pattern,
                "title_template": entity.title_template,
                "sort_order": entity.sort_order,
                "is_active": false,
            })),
        )
        .await;
        assert_invalid_field(status, &body, "is_active");

        let (status, body) = call(
            &pool,
            Method::DELETE,
            &format!("/api/v1/entities/{key}?version={}", entity.version),
            &token,
            None,
        )
        .await;
        assert_invalid_field(status, &body, "is_system");

        let after = get_entity(&pool, &token, key).await.entity;
        assert!(after.is_active, "{key}");
        assert_eq!(after.version, entity.version, "{key}");
    }
}

/// 系統欄位不可停用、不可變更型態；可以修改顯示名稱（migration 寫入的設定通過現有規則）。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn system_fields_cannot_be_deactivated_or_change_type(pool: PgPool) {
    let token = token_for(&pool, "admin", role::ADMIN).await;
    for (key, _, _, _) in SYSTEM_ENTITIES {
        let detail = get_entity(&pool, &token, key).await;
        for f in &detail.fields {
            let at = format!("{key}.{}", f.field_key);
            let (status, body) = call(
                &pool,
                Method::POST,
                &format!("/api/v1/entities/{key}/fields/{}/deactivate", f.field_key),
                &token,
                Some(json!({ "version": f.version })),
            )
            .await;
            assert_invalid_field(status, &body, "is_active");

            let (status, body) =
                patch_field(&pool, &token, key, f, json!({ "is_active": false })).await;
            assert_invalid_field(status, &body, "is_active");

            let (status, body) = call(
                &pool,
                Method::POST,
                &format!("/api/v1/entities/{key}/fields/{}/change-type", f.field_key),
                &token,
                Some(json!({ "version": f.version, "data_type": "long_text" })),
            )
            .await;
            assert_invalid_field(status, &body, "data_type");

            // 其他屬性不變，只改顯示名稱
            let (status, body) = patch_field(
                &pool,
                &token,
                key,
                f,
                json!({ "label": format!("{} 2", f.label) }),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{at}: {body}");
        }

        let after = get_entity(&pool, &token, key).await;
        for (before, after) in detail.fields.iter().zip(&after.fields) {
            assert!(after.is_active);
            assert_eq!(after.data_type, before.data_type);
            assert_eq!(after.label, format!("{} 2", before.label));
        }
    }
}

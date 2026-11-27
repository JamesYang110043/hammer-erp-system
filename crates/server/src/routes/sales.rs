//! 銷貨模組的專屬端點（docs/architecture.md §8.9、§10「專屬端點」）。
//!
//! | 端點 | 權限 | 成功回應 |
//! |---|---|---|
//! | `GET /api/v1/sales/product-lookup?customer_id=&q=&limit=` | `record.read` | `200` `Vec<SalesProductLookupItem>` |
//! | `GET /api/v1/sales/product-names?customer_id=&product_id=…` | `record.read` | `200` `Vec<SalesProductName>` |
//! | `GET /api/v1/products/{id}/customer-product-names` | `record.read` | `200` `Vec<ProductCustomerName>` |
//! | `POST /api/v1/sales/customer-product-names/batch` | `record.write` | `200` `CustomerProductNameBatchResult` |
//!
//! 格式見 [`hammer_shared::sales`]。

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use time::OffsetDateTime;

use super::auth::request_client_info;
use hammer_core::sales::{customer_name_batch, customer_names, product_lookup, product_names};
use hammer_shared::Id;
use hammer_shared::sales::{
    CustomerProductNameBatchRequest, CustomerProductNameBatchResult, ProductCustomerName,
    SalesProductLookupItem, SalesProductName,
};

use crate::{AppError, AppState, AuthUser};

/// `GET /api/v1/sales/product-lookup?customer_id=&q=&limit=`
///
/// 銷貨用商品選擇：比對商品標題與這位客戶的客戶品名（M11）。
pub async fn product_lookup(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Json<Vec<SalesProductLookupItem>>, AppError> {
    Ok(Json(
        product_lookup::product_lookup(&state.pool, &auth, &pairs).await?,
    ))
}

/// `GET /api/v1/sales/product-names?customer_id=&product_id=…`
///
/// 批次取得明細品名：一次取得多個商品依帶入順序決定的品名（M12）。
pub async fn product_names(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Json<Vec<SalesProductName>>, AppError> {
    Ok(Json(
        product_names::product_names(&state.pool, &auth, &pairs).await?,
    ))
}

/// `GET /api/v1/products/{id}/customer-product-names`
///
/// 商品的客戶品名：這個商品在各客戶的客戶品名（唯讀，M12）。
pub async fn product_customer_names(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Id>,
) -> Result<Json<Vec<ProductCustomerName>>, AppError> {
    Ok(Json(
        customer_names::product_customer_names(&state.pool, &auth, id).await?,
    ))
}

/// `POST /api/v1/sales/customer-product-names/batch`
///
/// 客戶品名批次寫入：依（客戶，商品）upsert，`dry_run` 只檢查（M12）。
pub async fn customer_product_names_batch(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Json(req): Json<CustomerProductNameBatchRequest>,
) -> Result<Json<CustomerProductNameBatchResult>, AppError> {
    let info = request_client_info(&headers);
    Ok(Json(
        customer_name_batch::batch_write(
            &state.pool,
            &auth,
            &req,
            info.as_deref(),
            OffsetDateTime::now_utc(),
        )
        .await?,
    ))
}

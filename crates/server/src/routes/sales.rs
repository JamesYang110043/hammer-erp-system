//! 銷貨模組的專屬端點（docs/architecture.md §8.9、§10「專屬端點」）。
//!
//! | 端點 | 權限 | 成功回應 |
//! |---|---|---|
//! | `GET /api/v1/sales/product-lookup?customer_id=&q=&limit=` | `record.read` | `200` `Vec<SalesProductLookupItem>` |
//! | `GET /api/v1/sales/product-names?customer_id=&product_id=…` | `record.read` | `200` `Vec<SalesProductName>` |
//!
//! 格式見 [`hammer_shared::sales`]。

use axum::Json;
use axum::extract::{Query, State};
use hammer_core::sales::{product_lookup, product_names};
use hammer_shared::sales::{SalesProductLookupItem, SalesProductName};

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

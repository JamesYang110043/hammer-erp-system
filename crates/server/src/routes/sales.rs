//! 銷貨模組的專屬端點（docs/architecture.md §8.9、§10「專屬端點」）。
//!
//! | 端點 | 權限 | 成功回應 |
//! |---|---|---|
//! | `GET /api/v1/sales/product-lookup?customer_id=&q=&limit=` | `record.read` | `200` `Vec<SalesProductLookupItem>` |
//!
//! 格式見 [`hammer_shared::sales`]。

use axum::Json;
use axum::extract::{Query, State};
use hammer_core::sales::product_lookup;
use hammer_shared::sales::SalesProductLookupItem;

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

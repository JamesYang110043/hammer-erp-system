//! 庫存查詢（docs/architecture.md §8.10「庫存查詢 API」、§10「專屬端點」）。
//!
//! 全部需要登入（[`AuthUser`]）；權限在 `hammer_core::inventory_queries` 檢查。
//! query / response 型別見 [`hammer_shared::inventory`]。
//!
//! | 端點 | 權限 | 成功回應 |
//! |---|---|---|
//! | `GET /api/v1/inventory/balances` | `record.read` | `200` `Page<StockBalance>` |
//! | `GET /api/v1/inventory/products/{product_id}/lots` | `record.read` | `200` `Page<InventoryLot>` |
//! | `GET /api/v1/inventory/transactions` | `record.read` | `200` `Page<InventoryTransaction>` |
//! | `POST /api/v1/inventory/allocation-preview` | `record.read` | `200` `AllocationPreview` |

use axum::Json;
use axum::extract::{Path, Query, State};
use hammer_core::inventory_queries;
use hammer_shared::Id;
use hammer_shared::inventory::{
    AllocationPreview, AllocationPreviewRequest, InventoryLot, InventoryTransaction, LotQuery,
    StockBalance, StockBalanceQuery, TransactionQuery,
};
use hammer_shared::pagination::Page;

use crate::{AppError, AppState, AuthUser};

/// `GET /api/v1/inventory/balances`
pub async fn list_balances(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(query): Query<StockBalanceQuery>,
) -> Result<Json<Page<StockBalance>>, AppError> {
    Ok(Json(
        inventory_queries::list_stock_balances(&state.pool, &auth, &query).await?,
    ))
}

/// `GET /api/v1/inventory/products/{product_id}/lots`
pub async fn list_product_lots(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(product_id): Path<Id>,
    Query(query): Query<LotQuery>,
) -> Result<Json<Page<InventoryLot>>, AppError> {
    Ok(Json(
        inventory_queries::list_product_lots(&state.pool, &auth, product_id, &query).await?,
    ))
}

/// `GET /api/v1/inventory/transactions`
pub async fn list_transactions(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(query): Query<TransactionQuery>,
) -> Result<Json<Page<InventoryTransaction>>, AppError> {
    Ok(Json(
        inventory_queries::list_transactions(&state.pool, &auth, &query).await?,
    ))
}

/// `POST /api/v1/inventory/allocation-preview`
pub async fn allocation_preview(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<AllocationPreviewRequest>,
) -> Result<Json<AllocationPreview>, AppError> {
    Ok(Json(
        inventory_queries::allocation_preview(&state.pool, &auth, &req).await?,
    ))
}

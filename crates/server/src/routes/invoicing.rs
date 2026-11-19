//! 發票模組的查詢 API（docs/architecture.md §8.11「查詢 API」、§10「專屬端點」）。
//!
//! 全部需要登入（[`AuthUser`]）；權限在 `hammer_core::invoicing::queries` 檢查。
//! query / response 型別見 [`hammer_shared::invoice`]。
//!
//! | 端點 | 權限 | 成功回應 |
//! |---|---|---|
//! | `GET /api/v1/invoicing/uninvoiced-shipments` | `record.read` | `200` `Page<UninvoicedShipment>` |
//! | `GET /api/v1/invoicing/uninvoiced-receipts` | `record.read` | `200` `Page<UninvoicedReceipt>` |
//! | `GET /api/v1/invoicing/links` | `record.read` | `200` `Vec<InvoiceLink>` |

use axum::Json;
use axum::extract::{Query, State};
use hammer_core::invoicing::queries;
use hammer_shared::invoice::{
    InvoiceLink, InvoiceLinkQuery, UninvoicedReceipt, UninvoicedReceiptQuery, UninvoicedShipment,
    UninvoicedShipmentQuery,
};
use hammer_shared::pagination::Page;

use crate::{AppError, AppState, AuthUser};

/// `GET /api/v1/invoicing/uninvoiced-shipments`
pub async fn list_uninvoiced_shipments(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(query): Query<UninvoicedShipmentQuery>,
) -> Result<Json<Page<UninvoicedShipment>>, AppError> {
    Ok(Json(
        queries::list_uninvoiced_shipments(&state.pool, &auth, &query).await?,
    ))
}

/// `GET /api/v1/invoicing/uninvoiced-receipts`
pub async fn list_uninvoiced_receipts(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(query): Query<UninvoicedReceiptQuery>,
) -> Result<Json<Page<UninvoicedReceipt>>, AppError> {
    Ok(Json(
        queries::list_uninvoiced_receipts(&state.pool, &auth, &query).await?,
    ))
}

/// `GET /api/v1/invoicing/links`
pub async fn list_links(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(query): Query<InvoiceLinkQuery>,
) -> Result<Json<Vec<InvoiceLink>>, AppError> {
    Ok(Json(queries::list_links(&state.pool, &auth, &query).await?))
}

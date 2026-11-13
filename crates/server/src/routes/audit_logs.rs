//! 操作紀錄查詢（docs/architecture.md §6.1、§8.4 `audit_logs`、§10 分頁）。
//!
//! 需要登入（[`AuthUser`]）與 `audit.read` 權限（在 `hammer_core::audit_logs` 檢查）。
//! request / response 型別與錯誤見 [`hammer_shared::audit`]。
//!
//! | 端點 | 成功回應 |
//! |---|---|
//! | `GET /api/v1/audit-logs` | `200` `Page<AuditLogEntry>` |

use axum::Json;
use axum::extract::{Query, State};
use hammer_core::audit_logs;
use hammer_shared::audit::{AuditLogEntry, AuditLogQuery};
use hammer_shared::pagination::Page;

use crate::{AppError, AppState, AuthUser};

/// `GET /api/v1/audit-logs`
pub async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(query): Query<AuditLogQuery>,
) -> Result<Json<Page<AuditLogEntry>>, AppError> {
    Ok(Json(
        audit_logs::list_audit_logs(&state.pool, &auth, &query).await?,
    ))
}

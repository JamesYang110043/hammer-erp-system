//! 系統單據的過帳與作廢（docs/architecture.md §7.1「系統單據」、§8.2、§9、§10「單據動作」）。
//!
//! - [`post_document`]：`draft` → `posted`
//! - [`void_document`]：`posted` → `void`（採購單：`confirmed` → `void`）
//! - [`confirm_document`]（只有採購單）：`draft` → `confirmed`
//! - [`close_document`]（只有採購單）：`confirmed` / `partially_received` → `closed`
//!
//! 各單據支援的動作與前後的狀態由處理函式決定（[`DocumentHandler::transition`]），不支援的動作 `404`。
//!
//! 都需要 `record.post`，request 為 [`DocumentActionRequest`]（只有 `version`），回應與單筆查詢相同
//! （[`crate::records::get_record`]，含 `lines` 與 `document`）。
//!
//! # 分派
//!
//! 共通的檢查、狀態變更與操作紀錄在這裡；各單據的過帳、作廢邏輯（庫存異動等）由該單據註冊的
//! [`DocumentHandler`] 處理，依資料類別代碼分派（[`DocumentHandlers`]）。沒有註冊處理函式的資料類別
//! 不支援過帳、作廢（`404`）。
//!
//! # 流程（同一個 transaction）
//!
//! 1. 權限 `record.post`（`403`）
//! 2. 資料類別必須是有註冊處理函式的系統單據，資料存在且未刪除，並以 `FOR UPDATE` 鎖定單據（`404`）
//! 3. `version`（`409 CONFLICT`）
//! 4. 狀態：必須是動作允許的前一個狀態（`422 INVALID_STATUS`）
//! 5. 處理函式（內容驗證、庫存，錯誤時整張 rollback）
//! 6. 更新 `status`、動作的時間與人員（`posted_*` / `voided_*` / `closed_*`），`version` + 1
//! 7. 操作紀錄：`action` 為 `post` / `void` / `confirm` / `close`，diff 為處理前後有變動的欄位（含 `status` 與處理函式寫入的
//!    值，明細的鍵為 `lines.<明細子表代碼>`）；`posted_at` 等不是欄位的值不記錄

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use hammer_db::document::{self as document_db, Stamp, StatusChange};
use hammer_db::sqlx::{PgConnection, PgPool};
use hammer_db::{DbError, ensure_version_matched};
use hammer_shared::document::{DocumentAction, DocumentActionRequest, DocumentStatus};
use hammer_shared::record::Record;
use hammer_shared::schema::{EntityDefinition, EntityKind};
use hammer_shared::{Id, permission};
use serde_json::{Map, Value};
use time::OffsetDateTime;

use crate::CoreError;
use crate::audit::AuditDiff;
use crate::auth::AuthContext;
use crate::purchasing::{
    GOODS_RECEIPT, GoodsReceiptHandler, PURCHASE_ORDER, PURCHASE_RETURN, PurchaseOrderHandler,
    PurchaseReturnHandler,
};
use crate::record_lines::{audit_key, audit_value};
use crate::records::{
    Detail, commit, detail_lines, document_status, fetch_record, invalid_status,
    load_details_for_write, load_target_for_write, lock_record, record_audit,
};
use crate::schema::{ENTITY_NOT_FOUND, ensure_version};
use crate::stock_adjustments::StockAdjustmentHandler;

/// 盤點調整單的資料類別代碼（§8.10）。
pub const STOCK_ADJUSTMENT: &str = "stock_adjustment";

/// 處理函式回傳的 future。
pub type HandlerFuture<'a> = Pin<Box<dyn Future<Output = Result<(), CoreError>> + Send + 'a>>;

/// 交給處理函式的單據。單據已鎖定（`FOR UPDATE`），`version` 與狀態已檢查。
#[derive(Debug)]
pub struct DocumentContext<'a> {
    /// 單據的資料類別（主表）。
    pub entity: &'a EntityDefinition,
    /// 單據（主表）的 id。
    pub record_id: Id,
    /// 單號。
    pub record_no: &'a str,
    /// 主表目前的 `data`（系統欄位與自訂欄位）。
    pub data: &'a Map<String, Value>,
    /// 操作者。
    pub actor: Id,
    /// 過帳、作廢的時間。
    pub now: OffsetDateTime,
    /// 用戶端資訊（處理函式寫入其他資料的操作紀錄時使用）。
    pub client_info: Option<&'a str>,
}

/// 一個動作的狀態轉換。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transition {
    /// 允許執行的狀態。
    pub from: &'static [DocumentStatus],
    /// 執行後的狀態。
    pub to: DocumentStatus,
    /// 寫入時間與人員的欄位。
    pub stamp: Stamp,
}

/// 一般單據（影響庫存）的狀態轉換：過帳 `draft` → `posted`、作廢 `posted` → `void`；不支援確認、結案。
pub fn standard_transition(action: DocumentAction) -> Option<Transition> {
    match action {
        DocumentAction::Post => Some(Transition {
            from: &[DocumentStatus::Draft],
            to: DocumentStatus::Posted,
            stamp: Stamp::Posted,
        }),
        DocumentAction::Void => Some(Transition {
            from: &[DocumentStatus::Posted],
            to: DocumentStatus::Void,
            stamp: Stamp::Voided,
        }),
        DocumentAction::Confirm | DocumentAction::Close => None,
    }
}

/// 一種單據的過帳、作廢等邏輯。在呼叫端的 transaction 內執行；回傳錯誤時整張單據 rollback。
///
/// 處理函式不更新主表的 `status`、`version`（由 [`post_document`] 等更新）；可以修改
/// 明細與主表的其他欄位（例如過帳時寫入的成本），變動會記在操作紀錄的 diff。
pub trait DocumentHandler: Send + Sync {
    /// 動作的狀態轉換；`None` 為不支援（`404`）。預設為 [`standard_transition`]。
    fn transition(&self, action: DocumentAction) -> Option<Transition> {
        standard_transition(action)
    }

    /// 過帳：驗證內容、寫入庫存異動等。
    fn post<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        doc: &'a DocumentContext<'a>,
    ) -> HandlerFuture<'a>;

    /// 作廢：反向處理過帳的結果。
    fn void<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        doc: &'a DocumentContext<'a>,
    ) -> HandlerFuture<'a>;

    /// 確認（[`Self::transition`] 支援時才會呼叫）。預設不做任何處理。
    fn confirm<'a>(
        &'a self,
        _conn: &'a mut PgConnection,
        _doc: &'a DocumentContext<'a>,
    ) -> HandlerFuture<'a> {
        Box::pin(async { Ok(()) })
    }

    /// 結案（[`Self::transition`] 支援時才會呼叫）。預設不做任何處理。
    fn close<'a>(
        &'a self,
        _conn: &'a mut PgConnection,
        _doc: &'a DocumentContext<'a>,
    ) -> HandlerFuture<'a> {
        Box::pin(async { Ok(()) })
    }
}

/// 不做任何處理的處理函式：只變更狀態與寫入操作紀錄。供測試使用。
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopHandler;

impl DocumentHandler for NoopHandler {
    fn post<'a>(
        &'a self,
        _conn: &'a mut PgConnection,
        _doc: &'a DocumentContext<'a>,
    ) -> HandlerFuture<'a> {
        Box::pin(async { Ok(()) })
    }

    fn void<'a>(
        &'a self,
        _conn: &'a mut PgConnection,
        _doc: &'a DocumentContext<'a>,
    ) -> HandlerFuture<'a> {
        Box::pin(async { Ok(()) })
    }
}

/// 各單據的處理函式，鍵為資料類別代碼。clone 成本低（內部是 `Arc`）。
#[derive(Clone, Default)]
pub struct DocumentHandlers {
    handlers: BTreeMap<String, Arc<dyn DocumentHandler>>,
}

impl DocumentHandlers {
    /// 沒有任何處理函式（所有資料類別都不支援過帳、作廢）。
    pub fn empty() -> Self {
        Self::default()
    }

    /// 系統內建的單據與其處理函式。
    pub fn standard() -> Self {
        Self::empty()
            .register(STOCK_ADJUSTMENT, StockAdjustmentHandler)
            .register(PURCHASE_ORDER, PurchaseOrderHandler)
            .register(GOODS_RECEIPT, GoodsReceiptHandler)
            .register(PURCHASE_RETURN, PurchaseReturnHandler)
    }

    /// 註冊（或取代）資料類別 `entity_key` 的處理函式。
    pub fn register(mut self, entity_key: &str, handler: impl DocumentHandler + 'static) -> Self {
        self.handlers
            .insert(entity_key.to_owned(), Arc::new(handler));
        self
    }

    /// 資料類別 `entity_key` 的處理函式。
    pub fn get(&self, entity_key: &str) -> Option<&dyn DocumentHandler> {
        self.handlers.get(entity_key).map(|h| h.as_ref())
    }
}

impl std::fmt::Debug for DocumentHandlers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_set().entries(self.handlers.keys()).finish()
    }
}

/// 過帳：`draft` → `posted`。需要 `record.post`。流程與錯誤見模組說明。
#[allow(clippy::too_many_arguments)]
pub async fn post_document(
    pool: &PgPool,
    handlers: &DocumentHandlers,
    ctx: &AuthContext,
    entity_key: &str,
    id: Id,
    req: &DocumentActionRequest,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<Record, CoreError> {
    run(
        pool,
        handlers,
        ctx,
        entity_key,
        id,
        DocumentAction::Post,
        req,
        client_info,
        now,
    )
    .await
}

/// 作廢：`posted` → `void`。需要 `record.post`。流程與錯誤見模組說明。
#[allow(clippy::too_many_arguments)]
pub async fn void_document(
    pool: &PgPool,
    handlers: &DocumentHandlers,
    ctx: &AuthContext,
    entity_key: &str,
    id: Id,
    req: &DocumentActionRequest,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<Record, CoreError> {
    run(
        pool,
        handlers,
        ctx,
        entity_key,
        id,
        DocumentAction::Void,
        req,
        client_info,
        now,
    )
    .await
}

/// 確認（只有採購單）：`draft` → `confirmed`。需要 `record.post`。流程與錯誤見模組說明。
#[allow(clippy::too_many_arguments)]
pub async fn confirm_document(
    pool: &PgPool,
    handlers: &DocumentHandlers,
    ctx: &AuthContext,
    entity_key: &str,
    id: Id,
    req: &DocumentActionRequest,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<Record, CoreError> {
    run(
        pool,
        handlers,
        ctx,
        entity_key,
        id,
        DocumentAction::Confirm,
        req,
        client_info,
        now,
    )
    .await
}

/// 手動結案（只有採購單）：`confirmed` / `partially_received` → `closed`。需要 `record.post`。
/// 流程與錯誤見模組說明。
#[allow(clippy::too_many_arguments)]
pub async fn close_document(
    pool: &PgPool,
    handlers: &DocumentHandlers,
    ctx: &AuthContext,
    entity_key: &str,
    id: Id,
    req: &DocumentActionRequest,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<Record, CoreError> {
    run(
        pool,
        handlers,
        ctx,
        entity_key,
        id,
        DocumentAction::Close,
        req,
        client_info,
        now,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run(
    pool: &PgPool,
    handlers: &DocumentHandlers,
    ctx: &AuthContext,
    entity_key: &str,
    id: Id,
    action: DocumentAction,
    req: &DocumentActionRequest,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<Record, CoreError> {
    ctx.require(permission::RECORD_POST)?;

    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let (entity, fields, store) = load_target_for_write(&mut tx, entity_key).await?;
    let handler = handlers
        .get(&entity.key)
        .filter(|_| entity.kind == EntityKind::Document && store.document_table().is_some())
        .ok_or_else(|| CoreError::NotFound(ENTITY_NOT_FOUND.into()))?;
    let transition = handler
        .transition(action)
        .ok_or_else(|| CoreError::NotFound(ENTITY_NOT_FOUND.into()))?;
    let table = store
        .document_table()
        .ok_or_else(|| CoreError::NotFound(ENTITY_NOT_FOUND.into()))?;

    let current = lock_record(&mut tx, &store, entity.id, id).await?;
    ensure_version(current.version, req.version)?;
    let status = document_status(&store, &current)?
        .ok_or_else(|| CoreError::Internal(format!("{} 不是系統單據", entity.key)))?;
    if !transition.from.contains(&status) {
        return Err(invalid_status(status, action.label()));
    }
    let record_no = current
        .record_no
        .clone()
        .ok_or_else(|| CoreError::Internal(format!("單據 {id} 沒有單號")))?;

    // 含已停用的明細子表：處理函式可能修改它們的明細
    let details = load_details_for_write(&mut tx, &entity).await?;
    let before = snapshot(&mut tx, &details, id, &current.data).await?;

    let data = current
        .data
        .as_object()
        .ok_or_else(|| CoreError::Internal("資料的 data 不是 JSON 物件".into()))?;
    let doc = DocumentContext {
        entity: &entity,
        record_id: id,
        record_no: &record_no,
        data,
        actor: ctx.user_id,
        now,
        client_info,
    };
    match action {
        DocumentAction::Post => handler.post(&mut tx, &doc).await?,
        DocumentAction::Void => handler.void(&mut tx, &doc).await?,
        DocumentAction::Confirm => handler.confirm(&mut tx, &doc).await?,
        DocumentAction::Close => handler.close(&mut tx, &doc).await?,
    }

    let change = StatusChange {
        id,
        version: req.version,
        at: now,
        by: ctx.user_id,
    };
    let from: Vec<&str> = transition.from.iter().map(|s| s.as_str()).collect();
    let result = document_db::mark_status(
        &mut *tx,
        table,
        &change,
        &from,
        transition.to.as_str(),
        transition.stamp,
    )
    .await?;
    ensure_version_matched(result)?;

    let updated = store
        .find(&mut tx, entity.id, id)
        .await?
        .ok_or_else(|| CoreError::Internal("寫入後找不到資料".into()))?;
    let after = snapshot(&mut tx, &details, id, &updated.data).await?;
    record_audit(
        &mut tx,
        ctx,
        action.as_str(),
        &entity.key,
        id,
        AuditDiff::between(Some(&before), Some(&after))?,
        client_info,
        now,
    )
    .await?;
    let record = fetch_record(&mut tx, &entity, &fields, &store, &details, id).await?;
    commit(tx).await?;

    tracing::info!(
        actor = %ctx.user_id,
        entity = %entity.key,
        record_id = %id,
        action = action.as_str(),
        "已變更單據狀態"
    );
    Ok(record)
}

/// 操作紀錄比對用的內容：主表的 `data`，加上每個有明細的明細子表 `lines.<代碼>`。
async fn snapshot(
    conn: &mut PgConnection,
    details: &[Detail],
    id: Id,
    data: &Value,
) -> Result<Value, CoreError> {
    let mut value = data.clone();
    let lines = detail_lines(conn, details, id).await?;
    if let Value::Object(map) = &mut value {
        for (detail, rows) in details.iter().zip(&lines) {
            if !rows.is_empty() {
                map.insert(
                    audit_key(&detail.entity.key),
                    audit_value(rows.iter().map(|r| (r.id, &r.data))),
                );
            }
        }
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_handlers_cover_system_documents() {
        let handlers = DocumentHandlers::standard();
        assert!(handlers.get(STOCK_ADJUSTMENT).is_some());
        assert!(handlers.get(PURCHASE_ORDER).is_some());
        assert!(handlers.get(GOODS_RECEIPT).is_some());
        assert!(handlers.get(PURCHASE_RETURN).is_some());
        assert!(handlers.get("customer").is_none());
        assert!(DocumentHandlers::empty().get(STOCK_ADJUSTMENT).is_none());
        assert_eq!(
            format!("{handlers:?}"),
            "{\"goods_receipt\", \"purchase_order\", \"purchase_return\", \"stock_adjustment\"}"
        );
    }

    #[test]
    fn standard_documents_post_and_void_only() {
        let post = standard_transition(DocumentAction::Post).unwrap();
        assert_eq!(post.from, [DocumentStatus::Draft]);
        assert_eq!(post.to, DocumentStatus::Posted);
        assert_eq!(post.stamp, Stamp::Posted);
        let void = standard_transition(DocumentAction::Void).unwrap();
        assert_eq!(void.from, [DocumentStatus::Posted]);
        assert_eq!(void.to, DocumentStatus::Void);
        assert_eq!(void.stamp, Stamp::Voided);
        assert!(standard_transition(DocumentAction::Confirm).is_none());
        assert!(standard_transition(DocumentAction::Close).is_none());
        let handler = StockAdjustmentHandler;
        assert_eq!(handler.transition(DocumentAction::Post), Some(post));
        assert!(handler.transition(DocumentAction::Close).is_none());
    }
}

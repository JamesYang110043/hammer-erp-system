//! 發票模組四種單據共用的作廢日期與原因（docs/architecture.md §8.11「開立與作廢」）。
//!
//! - [`void_details`]：讀取 request 的 `void_reason`（必填）、`void_date`（省略時為作廢當天的
//!   Asia/Taipei 日期，不可早於單據日期）
//! - [`write_void_details`]：檢查並寫入作廢日期與原因。在處理函式的 `void` 內、框架變更狀態之前寫入
//!   （migration 0015：已開立的單據可以先有作廢日期，已作廢的一定要有）

use hammer_db::invoicing::{self as db, InvoiceTable};
use hammer_db::sqlx::PgConnection;
use hammer_shared::time::TAIPEI_OFFSET;
use time::Date;

use super::save::{date_of, field_error};
use crate::CoreError;
use crate::documents::{DocumentContext, VOID_DATE, VOID_REASON};
use crate::record_data::FieldError;
use crate::record_lines::LineError;

/// 沒有作廢原因。
pub const MSG_VOID_REASON_REQUIRED: &str = "請輸入作廢原因";

/// 作廢的日期與原因。`date_field` 為單據日期的欄位；作廢日期早於單據日期時以 `early_message` 回報。
pub fn void_details(
    doc: &DocumentContext<'_>,
    date_field: &str,
    early_message: &str,
) -> Result<(Date, String), CoreError> {
    let reason = doc
        .request
        .void_reason
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let void_date = doc
        .request
        .void_date
        .unwrap_or_else(|| doc.now.to_offset(TAIPEI_OFFSET).date());
    let mut errors = Vec::new();
    if reason.is_none() {
        errors.push(field_error(VOID_REASON, MSG_VOID_REASON_REQUIRED));
    }
    if date_of(doc.data.get(date_field)).is_some_and(|d| void_date < d) {
        errors.push(field_error(VOID_DATE, early_message));
    }
    match reason {
        Some(reason) if errors.is_empty() => Ok((void_date, reason)),
        _ => Err(CoreError::InvalidFields(errors)),
    }
}

/// 檢查作廢日期與原因（[`void_details`]）並寫入主表。作廢的其他檢查失敗時整張 rollback。
pub async fn write_void_details(
    conn: &mut PgConnection,
    doc: &DocumentContext<'_>,
    table: InvoiceTable,
    date_field: &str,
    early_message: &str,
) -> Result<(), CoreError> {
    let (void_date, reason) = void_details(doc, date_field, early_message)?;
    db::set_void_details(&mut *conn, table, doc.record_id, void_date, &reason).await?;
    Ok(())
}

/// 欄位與明細錯誤 → `422 VALIDATION`（沒有錯誤時 `Ok`）。
pub fn validation(fields: Vec<FieldError>, lines: Vec<LineError>) -> Result<(), CoreError> {
    match (fields.is_empty(), lines.is_empty()) {
        (true, true) => Ok(()),
        (false, true) => Err(CoreError::InvalidFields(fields)),
        _ => Err(CoreError::InvalidLines { fields, lines }),
    }
}

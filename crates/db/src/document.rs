//! 系統單據主表的狀態與過帳、作廢資訊（docs/architecture.md §7.1「系統單據」、§8.2、§10「單據動作」）。
//!
//! 系統單據的主表（例如 `stock_adjustments`）除了通用資料 API 讀寫的欄位（[`crate::system_record`]），
//! 另有 `status` 與不是欄位的 `posted_at`、`posted_by`、`voided_at`、`voided_by`：
//!
//! - [`find_states`]：讀取一批單據的狀態與過帳、作廢資訊（回應的 `document`）
//! - [`mark_posted`]、[`mark_voided`]：過帳、作廢時更新狀態並遞增 `version`
//!
//! 資料表名稱來自 [`SystemTable`]（已檢查過名稱），一律加上引號拼接；值以 bind 參數傳入。

use sqlx::postgres::PgQueryResult;
use sqlx::{PgExecutor, Postgres, QueryBuilder};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::DbError;
use crate::system_record::SystemTable;

/// 一張單據的狀態與過帳、作廢資訊。
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct DocumentStateRow {
    pub id: Uuid,
    /// `draft` / `posted` / `void`。
    pub status: String,
    pub posted_at: Option<OffsetDateTime>,
    pub posted_by: Option<Uuid>,
    pub voided_at: Option<OffsetDateTime>,
    pub voided_by: Option<Uuid>,
}

/// `ids` 中未刪除的單據的狀態與過帳、作廢資訊（順序不固定）。
pub async fn find_states<'e>(
    executor: impl PgExecutor<'e>,
    table: &SystemTable,
    ids: &[Uuid],
) -> Result<Vec<DocumentStateRow>, DbError> {
    let mut qb = states_query(table, ids);
    Ok(qb
        .build_query_as::<DocumentStateRow>()
        .fetch_all(executor)
        .await?)
}

fn states_query(table: &SystemTable, ids: &[Uuid]) -> QueryBuilder<Postgres> {
    let mut qb =
        QueryBuilder::new("SELECT id, status, posted_at, posted_by, voided_at, voided_by FROM \"");
    qb.push(table.name())
        .push("\" WHERE id = ANY(")
        .push_bind(ids.to_vec())
        .push(") AND deleted_at IS NULL");
    qb
}

/// 狀態變更所需的資料。
#[derive(Debug, Clone, Copy)]
pub struct StatusChange {
    pub id: Uuid,
    /// 修改前的版本號（§9.4）。
    pub version: i32,
    /// 過帳或作廢的時間。
    pub at: OffsetDateTime,
    /// 過帳或作廢的人員。
    pub by: Uuid,
}

/// 過帳：`draft` → `posted`，寫入 `posted_at`、`posted_by`，遞增 `version`。版本號不符或狀態不是
/// `draft` 時影響 0 筆。
pub async fn mark_posted<'e>(
    executor: impl PgExecutor<'e>,
    table: &SystemTable,
    change: &StatusChange,
) -> Result<PgQueryResult, DbError> {
    let mut qb = status_query(table, change, "posted", "posted", "draft");
    Ok(qb.build().execute(executor).await?)
}

/// 作廢：`posted` → `void`，寫入 `voided_at`、`voided_by`，遞增 `version`。版本號不符或狀態不是
/// `posted` 時影響 0 筆。
pub async fn mark_voided<'e>(
    executor: impl PgExecutor<'e>,
    table: &SystemTable,
    change: &StatusChange,
) -> Result<PgQueryResult, DbError> {
    let mut qb = status_query(table, change, "void", "voided", "posted");
    Ok(qb.build().execute(executor).await?)
}

/// `prefix` 為 `posted` / `voided`（常數，不是使用者輸入）。
fn status_query(
    table: &SystemTable,
    change: &StatusChange,
    to: &'static str,
    prefix: &'static str,
    from: &'static str,
) -> QueryBuilder<Postgres> {
    let mut qb = QueryBuilder::new("UPDATE \"");
    qb.push(table.name())
        .push("\" SET status = ")
        .push_bind(to)
        .push(", ")
        .push(prefix)
        .push("_at = ")
        .push_bind(change.at)
        .push(", ")
        .push(prefix)
        .push("_by = ")
        .push_bind(change.by)
        .push(", updated_by = ")
        .push_bind(change.by)
        .push(", version = version + 1 WHERE id = ")
        .push_bind(change.id)
        .push(" AND version = ")
        .push_bind(change.version)
        .push(" AND status = ")
        .push_bind(from)
        .push(" AND deleted_at IS NULL");
    qb
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> SystemTable {
        SystemTable::new(Uuid::nil(), "stock_adjustments", Vec::new())
            .unwrap()
            .document()
    }

    fn change() -> StatusChange {
        StatusChange {
            id: Uuid::nil(),
            version: 1,
            at: OffsetDateTime::UNIX_EPOCH,
            by: Uuid::nil(),
        }
    }

    #[test]
    fn states_query_reads_document_columns() {
        let qb = states_query(&table(), &[Uuid::nil()]);
        assert_eq!(
            qb.sql().as_str(),
            "SELECT id, status, posted_at, posted_by, voided_at, voided_by \
             FROM \"stock_adjustments\" WHERE id = ANY($1) AND deleted_at IS NULL"
        );
    }

    #[test]
    fn post_and_void_check_version_and_status() {
        let qb = status_query(&table(), &change(), "posted", "posted", "draft");
        assert_eq!(
            qb.sql().as_str(),
            "UPDATE \"stock_adjustments\" SET status = $1, posted_at = $2, posted_by = $3, \
             updated_by = $4, version = version + 1 WHERE id = $5 AND version = $6 \
             AND status = $7 AND deleted_at IS NULL"
        );
        let qb = status_query(&table(), &change(), "void", "voided", "posted");
        assert!(qb.sql().as_str().contains("voided_at = $2, voided_by = $3"));
    }
}

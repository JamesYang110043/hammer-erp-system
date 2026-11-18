//! 系統單據主表的狀態與過帳、作廢資訊（docs/architecture.md §7.1「系統單據」、§8.2、§10「單據動作」）。
//!
//! 系統單據的主表（例如 `stock_adjustments`）除了通用資料 API 讀寫的欄位（[`crate::system_record`]），
//! 另有 `status` 與不是欄位的 `posted_at`、`posted_by`、`voided_at`、`voided_by`（採購單另有
//! `closed_at`、`closed_by`）：
//!
//! - [`find_states`]：讀取一批單據的狀態與過帳、作廢、結案資訊（回應的 `document`）
//! - [`mark_status`]：過帳、確認、作廢、結案時更新狀態、寫入時間與人員並遞增 `version`
//! - [`find_record_no`]：依資料表名稱與 id 讀取單號（含已刪除），供錯誤訊息指出是哪張單據
//!
//! 資料表名稱來自 [`SystemTable`]（已檢查過名稱），一律加上引號拼接；值以 bind 參數傳入。

use sqlx::postgres::PgQueryResult;
use sqlx::{PgExecutor, Postgres, QueryBuilder};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::DbError;
use crate::dynamic_index::is_safe_identifier;
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
    /// 手動結案（只有採購單有這兩個欄位，其他單據為 `None`）。
    pub closed_at: Option<OffsetDateTime>,
    pub closed_by: Option<Uuid>,
}

/// `ids` 中未刪除的單據的狀態與過帳、作廢、結案資訊（順序不固定）。
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
    // closed_at、closed_by 只有採購單有：以 to_jsonb 讀取，沒有這兩個欄位的資料表為 NULL
    let mut qb = QueryBuilder::new(
        "SELECT id, status, posted_at, posted_by, voided_at, voided_by, \
         (to_jsonb(t) ->> 'closed_at')::timestamptz AS closed_at, \
         (to_jsonb(t) ->> 'closed_by')::uuid AS closed_by FROM \"",
    );
    qb.push(table.name())
        .push("\" t WHERE id = ANY(")
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
    /// 動作的時間。
    pub at: OffsetDateTime,
    /// 執行動作的人員。
    pub by: Uuid,
}

/// 狀態變更時寫入時間與人員的欄位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stamp {
    /// `posted_at`、`posted_by`（過帳；採購單的確認）。
    Posted,
    /// `voided_at`、`voided_by`（作廢）。
    Voided,
    /// `closed_at`、`closed_by`（採購單的手動結案）。
    Closed,
}

impl Stamp {
    fn prefix(self) -> &'static str {
        match self {
            Self::Posted => "posted",
            Self::Voided => "voided",
            Self::Closed => "closed",
        }
    }
}

/// 狀態從 `from` 之一變為 `to`，寫入 `stamp` 的時間與人員，遞增 `version`。版本號不符或狀態不在 `from`
/// 中時影響 0 筆。`from`、`to` 為狀態的值（例如 `draft`、`posted`）。
pub async fn mark_status<'e>(
    executor: impl PgExecutor<'e>,
    table: &SystemTable,
    change: &StatusChange,
    from: &[&str],
    to: &str,
    stamp: Stamp,
) -> Result<PgQueryResult, DbError> {
    let mut qb = status_query(table, change, from, to, stamp);
    Ok(qb.build().execute(executor).await?)
}

fn status_query(
    table: &SystemTable,
    change: &StatusChange,
    from: &[&str],
    to: &str,
    stamp: Stamp,
) -> QueryBuilder<Postgres> {
    let prefix = stamp.prefix();
    let mut qb = QueryBuilder::new("UPDATE \"");
    qb.push(table.name())
        .push("\" SET status = ")
        .push_bind(to.to_owned())
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
        .push(" AND status = ANY(")
        .push_bind(from.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
        .push(") AND deleted_at IS NULL");
    qb
}

/// 系統單據主表 `table_name` 中 `id` 的單號（含已刪除）；找不到時為 `None`。
///
/// `table_name` 不符合 [`is_safe_identifier`] → [`DbError::UnsafeIdentifier`]。
pub async fn find_record_no<'e>(
    executor: impl PgExecutor<'e>,
    table_name: &str,
    id: Uuid,
) -> Result<Option<String>, DbError> {
    let mut qb = record_no_query(table_name, id)?;
    Ok(qb
        .build_query_scalar::<String>()
        .fetch_optional(executor)
        .await?)
}

fn record_no_query(table_name: &str, id: Uuid) -> Result<QueryBuilder<Postgres>, DbError> {
    if !is_safe_identifier(table_name) {
        return Err(DbError::UnsafeIdentifier(table_name.to_owned()));
    }
    let mut qb = QueryBuilder::new("SELECT record_no FROM \"");
    qb.push(table_name).push("\" WHERE id = ").push_bind(id);
    Ok(qb)
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
    fn record_no_query_checks_table_name() {
        let qb = record_no_query("stock_adjustments", Uuid::nil()).unwrap();
        assert_eq!(
            qb.sql().as_str(),
            "SELECT record_no FROM \"stock_adjustments\" WHERE id = $1"
        );
        assert!(matches!(
            record_no_query("x\"; DROP TABLE users; --", Uuid::nil()),
            Err(DbError::UnsafeIdentifier(_))
        ));
    }

    #[test]
    fn states_query_reads_document_columns() {
        let qb = states_query(&table(), &[Uuid::nil()]);
        assert_eq!(
            qb.sql().as_str(),
            "SELECT id, status, posted_at, posted_by, voided_at, voided_by, \
             (to_jsonb(t) ->> 'closed_at')::timestamptz AS closed_at, \
             (to_jsonb(t) ->> 'closed_by')::uuid AS closed_by \
             FROM \"stock_adjustments\" t WHERE id = ANY($1) AND deleted_at IS NULL"
        );
    }

    #[test]
    fn status_change_checks_version_and_status() {
        let qb = status_query(&table(), &change(), &["draft"], "posted", Stamp::Posted);
        assert_eq!(
            qb.sql().as_str(),
            "UPDATE \"stock_adjustments\" SET status = $1, posted_at = $2, posted_by = $3, \
             updated_by = $4, version = version + 1 WHERE id = $5 AND version = $6 \
             AND status = ANY($7) AND deleted_at IS NULL"
        );
        let qb = status_query(&table(), &change(), &["posted"], "void", Stamp::Voided);
        assert!(qb.sql().as_str().contains("voided_at = $2, voided_by = $3"));
        let qb = status_query(
            &table(),
            &change(),
            &["confirmed", "partially_received"],
            "closed",
            Stamp::Closed,
        );
        assert!(qb.sql().as_str().contains("closed_at = $2, closed_by = $3"));
    }
}

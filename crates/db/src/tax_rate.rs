//! `tax_rates` 資料表的查詢（docs/architecture.md §8.6）。由 migration 寫入，M3 不提供修改。

use rust_decimal::Decimal;
use sqlx::PgExecutor;

use crate::DbError;

/// 稅別資料列。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaxRateRow {
    pub code: String,
    pub name: String,
    pub rate: Decimal,
}

/// 列出所有稅別，依稅率由高到低、再依代碼排序。
pub async fn list_tax_rates<'e>(executor: impl PgExecutor<'e>) -> Result<Vec<TaxRateRow>, DbError> {
    let rows = sqlx::query_as!(
        TaxRateRow,
        "SELECT code, name, rate FROM tax_rates ORDER BY rate DESC, code"
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

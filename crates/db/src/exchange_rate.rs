//! `exchange_rates` 資料表的存取（docs/architecture.md §8.6）。
//!
//! 規則（權限、幣別檢查、匯率格式）在 `hammer_core::exchange_rates`。

use rust_decimal::Decimal;
use sqlx::PgExecutor;
use sqlx::postgres::PgQueryResult;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::DbError;

/// 同一幣別同一天只能有一筆未刪除的匯率（部分唯一索引）。
pub const CURRENCY_DATE_UNIQUE: &str = "uq_exchange_rates_currency_date";

/// 匯率資料列（未刪除）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExchangeRateRow {
    pub id: Uuid,
    pub currency_code: String,
    pub rate_date: Date,
    pub rate: Decimal,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub version: i32,
}

/// 查詢匯率的篩選條件。`None` 代表不篩選該條件；日期區間含開始、含結束。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExchangeRateFilter<'a> {
    pub currency_code: Option<&'a str>,
    pub date_from: Option<Date>,
    pub date_to: Option<Date>,
}

/// 依條件查詢未刪除的匯率，依 `rate_date DESC, currency_code, id` 排序。
pub async fn list_exchange_rates<'e>(
    executor: impl PgExecutor<'e>,
    filter: &ExchangeRateFilter<'_>,
    limit: i64,
    offset: i64,
) -> Result<Vec<ExchangeRateRow>, DbError> {
    let rows = sqlx::query_as!(
        ExchangeRateRow,
        r#"
        SELECT id, currency_code, rate_date, rate, created_at, updated_at, version
        FROM exchange_rates
        WHERE deleted_at IS NULL
          AND ($1::text IS NULL OR currency_code = $1)
          AND ($2::date IS NULL OR rate_date >= $2)
          AND ($3::date IS NULL OR rate_date <= $3)
        ORDER BY rate_date DESC, currency_code, id
        LIMIT $4 OFFSET $5
        "#,
        filter.currency_code,
        filter.date_from,
        filter.date_to,
        limit,
        offset,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 符合條件的未刪除匯率筆數。條件與 [`list_exchange_rates`] 相同。
pub async fn count_exchange_rates<'e>(
    executor: impl PgExecutor<'e>,
    filter: &ExchangeRateFilter<'_>,
) -> Result<i64, DbError> {
    let n = sqlx::query_scalar!(
        r#"
        SELECT count(*) AS "n!"
        FROM exchange_rates
        WHERE deleted_at IS NULL
          AND ($1::text IS NULL OR currency_code = $1)
          AND ($2::date IS NULL OR rate_date >= $2)
          AND ($3::date IS NULL OR rate_date <= $3)
        "#,
        filter.currency_code,
        filter.date_from,
        filter.date_to,
    )
    .fetch_one(executor)
    .await?;
    Ok(n)
}

/// 依 id 查詢未刪除的匯率。
pub async fn find_exchange_rate<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
) -> Result<Option<ExchangeRateRow>, DbError> {
    let row = sqlx::query_as!(
        ExchangeRateRow,
        r#"
        SELECT id, currency_code, rate_date, rate, created_at, updated_at, version
        FROM exchange_rates
        WHERE id = $1 AND deleted_at IS NULL
        "#,
        id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 依 id 查詢未刪除的匯率並鎖定該列（`FOR UPDATE`）。
pub async fn find_exchange_rate_for_update<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
) -> Result<Option<ExchangeRateRow>, DbError> {
    let row = sqlx::query_as!(
        ExchangeRateRow,
        r#"
        SELECT id, currency_code, rate_date, rate, created_at, updated_at, version
        FROM exchange_rates
        WHERE id = $1 AND deleted_at IS NULL
        FOR UPDATE
        "#,
        id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 幣別 `currency_code` 在 `date`（含）之前、未刪除的匯率中日期最近的一筆：`(rate_date, rate)`。
pub async fn find_effective_rate<'e>(
    executor: impl PgExecutor<'e>,
    currency_code: &str,
    date: Date,
) -> Result<Option<(Date, Decimal)>, DbError> {
    let row = sqlx::query!(
        r#"
        SELECT rate_date, rate
        FROM exchange_rates
        WHERE currency_code = $1 AND rate_date <= $2 AND deleted_at IS NULL
        ORDER BY rate_date DESC
        LIMIT 1
        "#,
        currency_code,
        date,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row.map(|r| (r.rate_date, r.rate)))
}

/// 新增匯率所需的資料。
#[derive(Debug, Clone)]
pub struct NewExchangeRate<'a> {
    pub id: Uuid,
    pub currency_code: &'a str,
    pub rate_date: Date,
    pub rate: Decimal,
    pub created_by: Uuid,
}

/// 新增匯率。同一幣別同一天已有未刪除的匯率時回傳違反 [`CURRENCY_DATE_UNIQUE`] 的錯誤。
pub async fn insert_exchange_rate<'e>(
    executor: impl PgExecutor<'e>,
    new: &NewExchangeRate<'_>,
) -> Result<(), DbError> {
    sqlx::query!(
        r#"
        INSERT INTO exchange_rates (id, currency_code, rate_date, rate, created_by, updated_by)
        VALUES ($1, $2, $3, $4, $5, $5)
        "#,
        new.id,
        new.currency_code,
        new.rate_date,
        new.rate,
        new.created_by,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// 修改匯率（樂觀鎖定，交給 [`crate::ensure_version_matched`] 檢查）。
pub async fn update_exchange_rate<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    version: i32,
    rate: Decimal,
    updated_by: Uuid,
) -> Result<PgQueryResult, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE exchange_rates
        SET rate = $3, updated_by = $4, version = version + 1
        WHERE id = $1 AND version = $2 AND deleted_at IS NULL
        "#,
        id,
        version,
        rate,
        updated_by,
    )
    .execute(executor)
    .await?;
    Ok(result)
}

/// 軟刪除匯率（樂觀鎖定，交給 [`crate::ensure_version_matched`] 檢查）。
pub async fn soft_delete_exchange_rate<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    version: i32,
    deleted_at: OffsetDateTime,
    updated_by: Uuid,
) -> Result<PgQueryResult, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE exchange_rates
        SET deleted_at = $3, updated_by = $4, version = version + 1
        WHERE id = $1 AND version = $2 AND deleted_at IS NULL
        "#,
        id,
        version,
        deleted_at,
        updated_by,
    )
    .execute(executor)
    .await?;
    Ok(result)
}

//! `currencies` 資料表的存取（docs/architecture.md §8.6）。
//!
//! 幣別不可刪除（沒有 `deleted_at`），只能停用。規則在 `hammer_core::currencies`。

use sqlx::PgExecutor;
use sqlx::postgres::PgQueryResult;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::DbError;

/// 幣別代碼的唯一限制名稱（`code TEXT NOT NULL UNIQUE`）。
pub const CODE_UNIQUE: &str = "currencies_code_key";

/// 幣別資料列。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrencyRow {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub symbol: Option<String>,
    pub decimal_places: i16,
    pub is_base: bool,
    pub is_active: bool,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub version: i32,
}

/// 列出幣別，本位幣優先、再依代碼排序。`include_inactive = false` 時只列出啟用中的。
pub async fn list_currencies<'e>(
    executor: impl PgExecutor<'e>,
    include_inactive: bool,
) -> Result<Vec<CurrencyRow>, DbError> {
    let rows = sqlx::query_as!(
        CurrencyRow,
        r#"
        SELECT id, code, name, symbol, decimal_places, is_base, is_active,
               created_at, updated_at, version
        FROM currencies
        WHERE $1 OR is_active
        ORDER BY is_base DESC, code
        "#,
        include_inactive,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 依代碼查詢幣別（含已停用的）。
pub async fn find_currency<'e>(
    executor: impl PgExecutor<'e>,
    code: &str,
) -> Result<Option<CurrencyRow>, DbError> {
    let row = sqlx::query_as!(
        CurrencyRow,
        r#"
        SELECT id, code, name, symbol, decimal_places, is_base, is_active,
               created_at, updated_at, version
        FROM currencies
        WHERE code = $1
        "#,
        code,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 依代碼查詢幣別並鎖定該列（`FOR UPDATE`），用於修改幣別。
pub async fn find_currency_for_update<'e>(
    executor: impl PgExecutor<'e>,
    code: &str,
) -> Result<Option<CurrencyRow>, DbError> {
    let row = sqlx::query_as!(
        CurrencyRow,
        r#"
        SELECT id, code, name, symbol, decimal_places, is_base, is_active,
               created_at, updated_at, version
        FROM currencies
        WHERE code = $1
        FOR UPDATE
        "#,
        code,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 依代碼查詢幣別並加上共享鎖（`FOR SHARE`），用於新增、修改匯率：
/// 避免同時停用幣別時，仍為已停用的幣別寫入匯率。
pub async fn find_currency_for_share<'e>(
    executor: impl PgExecutor<'e>,
    code: &str,
) -> Result<Option<CurrencyRow>, DbError> {
    let row = sqlx::query_as!(
        CurrencyRow,
        r#"
        SELECT id, code, name, symbol, decimal_places, is_base, is_active,
               created_at, updated_at, version
        FROM currencies
        WHERE code = $1
        FOR SHARE
        "#,
        code,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 新增幣別所需的資料（非本位幣、啟用中）。
#[derive(Debug, Clone)]
pub struct NewCurrency<'a> {
    pub id: Uuid,
    pub code: &'a str,
    pub name: &'a str,
    pub symbol: Option<&'a str>,
    pub decimal_places: i16,
    pub created_by: Uuid,
}

/// 新增幣別。代碼重複時回傳違反 [`CODE_UNIQUE`] 的錯誤。
pub async fn insert_currency<'e>(
    executor: impl PgExecutor<'e>,
    new: &NewCurrency<'_>,
) -> Result<(), DbError> {
    sqlx::query!(
        r#"
        INSERT INTO currencies (id, code, name, symbol, decimal_places, created_by, updated_by)
        VALUES ($1, $2, $3, $4, $5, $6, $6)
        "#,
        new.id,
        new.code,
        new.name,
        new.symbol,
        new.decimal_places,
        new.created_by,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// 修改幣別的內容。
#[derive(Debug, Clone)]
pub struct CurrencyUpdate<'a> {
    pub id: Uuid,
    /// 讀取時的版本號。
    pub version: i32,
    pub name: &'a str,
    pub symbol: Option<&'a str>,
    pub decimal_places: i16,
    pub is_active: bool,
    pub updated_by: Uuid,
}

/// 修改幣別（樂觀鎖定，交給 [`crate::ensure_version_matched`] 檢查）。
pub async fn update_currency<'e>(
    executor: impl PgExecutor<'e>,
    update: &CurrencyUpdate<'_>,
) -> Result<PgQueryResult, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE currencies
        SET name = $3, symbol = $4, decimal_places = $5, is_active = $6,
            updated_by = $7, version = version + 1
        WHERE id = $1 AND version = $2
        "#,
        update.id,
        update.version,
        update.name,
        update.symbol,
        update.decimal_places,
        update.is_active,
        update.updated_by,
    )
    .execute(executor)
    .await?;
    Ok(result)
}

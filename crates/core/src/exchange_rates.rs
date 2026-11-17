//! 匯率（docs/architecture.md §6.1、§8.6 `exchange_rates`、§9.4、§10 分頁）。
//!
//! 權限（Q5）：查看需要 `record.read`；新增、修改、刪除需要 `record.write`。
//!
//! - [`list_exchange_rates`]：依幣別與日期區間分頁查詢未刪除的匯率
//! - [`effective_rate`]：單據日期當天或之前最近的一筆匯率
//! - [`create_exchange_rate`]、[`update_exchange_rate`]、[`delete_exchange_rate`]：
//!   在單一 transaction 內完成變更與操作紀錄
//!
//! 幣別必須存在、啟用中、不是本位幣才能新增、修改匯率；讀取幣別時加上共享鎖，
//! 避免與停用幣別同時進行時仍寫入已停用幣別的匯率。
//! 操作紀錄的 `entity_key` 為 [`ENTITY_KEY`]、`record_id` 為匯率的 `id`。

use hammer_db::currency::{self as currency_db, CurrencyRow};
use hammer_db::exchange_rate::{
    self as db, CURRENCY_DATE_UNIQUE, ExchangeRateFilter, ExchangeRateRow, NewExchangeRate,
};
use hammer_db::sqlx::{PgPool, PgTransaction};
use hammer_db::{DbError, ensure_version_matched};
use hammer_shared::decimal::Decimal;
use hammer_shared::exchange_rate::{
    EffectiveRate, ExchangeRate, ExchangeRateQuery, RATE_SCALE, field,
};
use hammer_shared::pagination::{Page, PageRequest};
use hammer_shared::{Id, audit as action, new_id, permission};
use serde_json::{Value, json};
use time::macros::format_description;
use time::{Date, OffsetDateTime};

use crate::CoreError;
use crate::audit::{self, AuditDiff, AuditEntry};
use crate::auth::AuthContext;
use crate::currencies::normalize_code;

/// 操作紀錄的 `entity_key`。
pub const ENTITY_KEY: &str = "exchange_rates";

/// 匯率不存在或已刪除時的錯誤訊息。
pub const RATE_NOT_FOUND: &str = "找不到此匯率";

/// `NUMERIC(18,6)` 整數部分最多 12 位數。
const MAX_INTEGER_DIGITS: u32 = 18 - RATE_SCALE;

// ---------------------------------------------------------------------------
// 純邏輯
// ---------------------------------------------------------------------------

/// 檢查匯率：大於 0、最多 6 位小數（不自動四捨五入）、整數部分不超過 12 位。
pub fn validate_rate(rate: Decimal) -> Result<Decimal, CoreError> {
    if rate <= Decimal::ZERO {
        return Err(invalid(field::RATE, "匯率必須大於 0"));
    }
    if rate.normalize().scale() > RATE_SCALE {
        return Err(invalid(
            field::RATE,
            &format!("匯率最多 {RATE_SCALE} 位小數"),
        ));
    }
    if rate.trunc() >= Decimal::from(10_i64.pow(MAX_INTEGER_DIGITS)) {
        return Err(invalid(field::RATE, "匯率超過可輸入的範圍"));
    }
    Ok(rate)
}

/// 新增、修改匯率時檢查幣別：必須啟用中、不是本位幣。
pub fn ensure_rate_editable(currency: &CurrencyRow) -> Result<(), CoreError> {
    if currency.is_base {
        return Err(invalid(field::CURRENCY_CODE, "本位幣不需要設定匯率"));
    }
    if !currency.is_active {
        return Err(invalid(
            field::CURRENCY_CODE,
            &format!("幣別 {} 已停用，不可新增或修改匯率", currency.code),
        ));
    }
    Ok(())
}

/// 本位幣的匯率（固定 1，6 位小數）。
pub fn base_rate() -> Decimal {
    Decimal::new(1_000_000, RATE_SCALE)
}

/// 資料列轉成 API 回應。
pub fn to_exchange_rate(row: ExchangeRateRow) -> ExchangeRate {
    ExchangeRate {
        id: row.id,
        currency_code: row.currency_code,
        rate_date: row.rate_date,
        rate: row.rate,
        created_at: row.created_at,
        updated_at: row.updated_at,
        version: row.version,
    }
}

/// 檢查過的列表查詢條件。
#[derive(Debug, Clone, PartialEq, Eq)]
struct ValidQuery {
    currency_code: Option<String>,
    date_from: Option<Date>,
    date_to: Option<Date>,
    page: PageRequest,
}

fn validate_query(query: &ExchangeRateQuery) -> Result<ValidQuery, CoreError> {
    let page =
        PageRequest::new(query.page, query.page_size).map_err(|err| CoreError::InvalidField {
            field: err.field,
            message: err.message().to_owned(),
        })?;
    if let (Some(from), Some(to)) = (query.date_from, query.date_to)
        && from > to
    {
        return Err(invalid(field::DATE_FROM, "開始日期不可晚於結束日期"));
    }
    Ok(ValidQuery {
        currency_code: query
            .currency_code
            .as_deref()
            .map(normalize_code)
            .filter(|c| !c.is_empty()),
        date_from: query.date_from,
        date_to: query.date_to,
        page,
    })
}

fn format_date(date: Date) -> String {
    date.format(format_description!("[year]-[month]-[day]"))
        .unwrap_or_default()
}

fn audit_json(row: &ExchangeRateRow) -> Value {
    json!({
        "currency_code": row.currency_code,
        "rate_date": format_date(row.rate_date),
        "rate": row.rate.to_string(),
    })
}

fn invalid(field: &'static str, message: &str) -> CoreError {
    CoreError::InvalidField {
        field,
        message: message.to_owned(),
    }
}

fn currency_not_found(code: &str) -> CoreError {
    if code.is_empty() {
        invalid(field::CURRENCY_CODE, "請選擇幣別")
    } else {
        invalid(field::CURRENCY_CODE, &format!("幣別 {code} 不存在"))
    }
}

fn not_found() -> CoreError {
    CoreError::NotFound(RATE_NOT_FOUND.into())
}

// ---------------------------------------------------------------------------
// 查詢
// ---------------------------------------------------------------------------

/// 依幣別與日期區間（含開始、含結束）分頁查詢未刪除的匯率。需要 `record.read`。
///
/// `date_from` 晚於 `date_to`、`page` 或 `page_size` 小於 1 → [`CoreError::InvalidField`]。
pub async fn list_exchange_rates(
    pool: &PgPool,
    ctx: &AuthContext,
    query: &ExchangeRateQuery,
) -> Result<Page<ExchangeRate>, CoreError> {
    ctx.require(permission::RECORD_READ)?;
    let valid = validate_query(query)?;
    let filter = ExchangeRateFilter {
        currency_code: valid.currency_code.as_deref(),
        date_from: valid.date_from,
        date_to: valid.date_to,
    };
    let total = db::count_exchange_rates(pool, &filter).await?;
    let rows =
        db::list_exchange_rates(pool, &filter, valid.page.limit(), valid.page.offset()).await?;
    let items = rows.into_iter().map(to_exchange_rate).collect();
    Ok(Page::new(
        items,
        u32::try_from(total).unwrap_or(u32::MAX),
        valid.page,
    ))
}

/// 單據日期 `date` 當天或之前最近的一筆匯率。需要 `record.read`。
///
/// - 找不到 → `rate`、`rate_date` 為 `None`（不是錯誤）
/// - 本位幣 → `rate` 固定為 1、`rate_date` 為 `None`
/// - 幣別不存在 → [`CoreError::InvalidField`]（`currency_code`）；已停用的幣別仍可查詢
pub async fn effective_rate(
    pool: &PgPool,
    ctx: &AuthContext,
    currency_code: &str,
    date: Date,
) -> Result<EffectiveRate, CoreError> {
    ctx.require(permission::RECORD_READ)?;
    let code = normalize_code(currency_code);
    let currency = currency_db::find_currency(pool, &code)
        .await?
        .ok_or_else(|| currency_not_found(&code))?;
    let (rate_date, rate) = if currency.is_base {
        (None, Some(base_rate()))
    } else {
        match db::find_effective_rate(pool, &currency.code, date).await? {
            Some((rate_date, rate)) => (Some(rate_date), Some(rate)),
            None => (None, None),
        }
    };
    Ok(EffectiveRate {
        currency_code: currency.code,
        date,
        rate,
        rate_date,
    })
}

// ---------------------------------------------------------------------------
// 寫入
// ---------------------------------------------------------------------------

/// 新增匯率的輸入。
#[derive(Debug, Clone, Copy)]
pub struct NewRateInput<'a> {
    pub currency_code: &'a str,
    pub rate_date: Date,
    pub rate: Decimal,
}

/// 新增匯率。需要 `record.write`。
///
/// 幣別不存在、已停用或為本位幣（`currency_code`）、匯率不合法（`rate`）、
/// 同一幣別同一天已有未刪除的匯率（`rate_date`）→ [`CoreError::InvalidField`]。
pub async fn create_exchange_rate(
    pool: &PgPool,
    ctx: &AuthContext,
    input: &NewRateInput<'_>,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<ExchangeRate, CoreError> {
    ctx.require(permission::RECORD_WRITE)?;
    let code = normalize_code(input.currency_code);
    let rate = validate_rate(input.rate)?;

    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let currency = currency_db::find_currency_for_share(&mut *tx, &code)
        .await?
        .ok_or_else(|| currency_not_found(&code))?;
    ensure_rate_editable(&currency)?;

    let id = new_id();
    db::insert_exchange_rate(
        &mut *tx,
        &NewExchangeRate {
            id,
            currency_code: &currency.code,
            rate_date: input.rate_date,
            rate,
            created_by: ctx.user_id,
        },
    )
    .await
    .map_err(|e| {
        if e.unique_violation() == Some(CURRENCY_DATE_UNIQUE) {
            invalid(
                field::RATE_DATE,
                &format!(
                    "{} 在 {} 已有匯率",
                    currency.code,
                    format_date(input.rate_date)
                ),
            )
        } else {
            e.into()
        }
    })?;
    let row = fetch(&mut tx, id).await?;
    record(
        &mut tx,
        ctx,
        action::CREATE,
        id,
        AuditDiff::between(None, Some(&audit_json(&row)))?,
        client_info,
        now,
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;

    tracing::info!(actor = %ctx.user_id, rate_id = %id, currency = %row.currency_code, "已新增匯率");
    Ok(to_exchange_rate(row))
}

/// 修改匯率（只能修改 `rate`）。需要 `record.write`。
///
/// - 不存在或已刪除 → [`CoreError::NotFound`]
/// - `version` 不符 → `Conflict`（§9.4）
/// - 幣別已停用（`currency_code`）、匯率不合法（`rate`）→ [`CoreError::InvalidField`]
/// - 匯率與目前相同時不寫入，回傳目前的資料（`version` 不變）
pub async fn update_exchange_rate(
    pool: &PgPool,
    ctx: &AuthContext,
    id: Id,
    version: i32,
    rate: Decimal,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<ExchangeRate, CoreError> {
    ctx.require(permission::RECORD_WRITE)?;
    let rate = validate_rate(rate)?;

    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let current = db::find_exchange_rate_for_update(&mut *tx, id)
        .await?
        .ok_or_else(not_found)?;
    if current.version != version {
        return Err(DbError::Conflict.into());
    }
    let currency = currency_db::find_currency_for_share(&mut *tx, &current.currency_code)
        .await?
        .ok_or_else(|| CoreError::Internal("匯率的幣別不存在".into()))?;
    ensure_rate_editable(&currency)?;
    if current.rate == rate {
        return Ok(to_exchange_rate(current));
    }

    ensure_version_matched(
        db::update_exchange_rate(&mut *tx, id, version, rate, ctx.user_id).await?,
    )?;
    let row = fetch(&mut tx, id).await?;
    record(
        &mut tx,
        ctx,
        action::UPDATE,
        id,
        AuditDiff::between(Some(&audit_json(&current)), Some(&audit_json(&row)))?,
        client_info,
        now,
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;

    tracing::info!(actor = %ctx.user_id, rate_id = %id, "已修改匯率");
    Ok(to_exchange_rate(row))
}

/// 軟刪除匯率。需要 `record.write`。刪除後同一幣別同一天可以重新新增。
///
/// - 不存在或已刪除 → [`CoreError::NotFound`]
/// - `version` 不符 → `Conflict`（§9.4）
pub async fn delete_exchange_rate(
    pool: &PgPool,
    ctx: &AuthContext,
    id: Id,
    version: i32,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<(), CoreError> {
    ctx.require(permission::RECORD_WRITE)?;

    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let current = db::find_exchange_rate_for_update(&mut *tx, id)
        .await?
        .ok_or_else(not_found)?;
    if current.version != version {
        return Err(DbError::Conflict.into());
    }
    ensure_version_matched(
        db::soft_delete_exchange_rate(&mut *tx, id, version, now, ctx.user_id).await?,
    )?;
    record(
        &mut tx,
        ctx,
        action::DELETE,
        id,
        AuditDiff::between(Some(&audit_json(&current)), None)?,
        client_info,
        now,
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;

    tracing::info!(actor = %ctx.user_id, rate_id = %id, "已刪除匯率");
    Ok(())
}

async fn record(
    tx: &mut PgTransaction<'_>,
    ctx: &AuthContext,
    action: &str,
    record_id: Id,
    diff: AuditDiff,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<(), CoreError> {
    audit::record(
        tx,
        &AuditEntry {
            user_id: Some(ctx.user_id),
            action,
            entity_key: Some(ENTITY_KEY),
            record_id: Some(record_id),
            diff,
            client_info,
            occurred_at: now,
        },
    )
    .await?;
    Ok(())
}

async fn fetch(tx: &mut PgTransaction<'_>, id: Id) -> Result<ExchangeRateRow, CoreError> {
    db::find_exchange_rate(&mut **tx, id)
        .await?
        .ok_or_else(|| CoreError::Internal("寫入後找不到匯率".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hammer_shared::ErrorCode;
    use hammer_shared::pagination::{MAX_PAGE_SIZE, field as page_field};
    use std::str::FromStr;
    use time::macros::{date, datetime};

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn field_of(err: &CoreError) -> &'static str {
        match err {
            CoreError::InvalidField { field, .. } => field,
            other => panic!("不是欄位錯誤：{other:?}"),
        }
    }

    fn currency(code: &str, is_base: bool, is_active: bool) -> CurrencyRow {
        CurrencyRow {
            id: new_id(),
            code: code.into(),
            name: code.into(),
            symbol: None,
            decimal_places: 2,
            is_base,
            is_active,
            created_at: datetime!(2026-11-17 00:00 UTC),
            updated_at: datetime!(2026-11-17 00:00 UTC),
            version: 1,
        }
    }

    #[test]
    fn valid_rates_are_kept_as_is() {
        for s in ["32.15", "0.000001", "0.2150000", "999999999999.999999", "1"] {
            assert_eq!(validate_rate(dec(s)).unwrap(), dec(s), "{s}");
        }
        // 不會被四捨五入或改變小數位數
        assert_eq!(validate_rate(dec("32.150")).unwrap().to_string(), "32.150");
    }

    #[test]
    fn invalid_rates_are_rejected() {
        for s in [
            "0",
            "-1",
            "-0.000001",
            "0.0000001",
            "32.1234567",
            "1000000000000",
        ] {
            let err = validate_rate(dec(s)).unwrap_err();
            assert_eq!(err.code(), ErrorCode::Validation);
            assert_eq!(field_of(&err), field::RATE, "{s}");
        }
    }

    #[test]
    fn rate_editable_only_for_active_foreign_currency() {
        assert!(ensure_rate_editable(&currency("USD", false, true)).is_ok());
        for c in [currency("TWD", true, true), currency("USD", false, false)] {
            let err = ensure_rate_editable(&c).unwrap_err();
            assert_eq!(field_of(&err), field::CURRENCY_CODE, "{}", c.code);
        }
    }

    #[test]
    fn base_rate_has_six_decimals() {
        assert_eq!(base_rate().to_string(), "1.000000");
    }

    #[test]
    fn query_defaults_and_normalization() {
        let valid = validate_query(&ExchangeRateQuery::default()).unwrap();
        assert_eq!(
            valid,
            ValidQuery {
                currency_code: None,
                date_from: None,
                date_to: None,
                page: PageRequest::default(),
            }
        );
        let valid = validate_query(&ExchangeRateQuery {
            currency_code: Some(" usd ".into()),
            date_from: Some(date!(2026 - 11 - 01)),
            date_to: Some(date!(2026 - 11 - 01)),
            page: Some(2),
            page_size: Some(1000),
        })
        .unwrap();
        assert_eq!(valid.currency_code.as_deref(), Some("USD"));
        assert_eq!(valid.page.page, 2);
        assert_eq!(valid.page.page_size, MAX_PAGE_SIZE);
        let valid = validate_query(&ExchangeRateQuery {
            currency_code: Some("  ".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(valid.currency_code, None);
    }

    #[test]
    fn invalid_query_is_rejected() {
        let err = validate_query(&ExchangeRateQuery {
            date_from: Some(date!(2026 - 11 - 02)),
            date_to: Some(date!(2026 - 11 - 01)),
            ..Default::default()
        })
        .unwrap_err();
        assert_eq!(field_of(&err), field::DATE_FROM);
        let err = validate_query(&ExchangeRateQuery {
            page: Some(0),
            ..Default::default()
        })
        .unwrap_err();
        assert_eq!(field_of(&err), page_field::PAGE);
    }

    #[test]
    fn audit_json_uses_strings() {
        let row = ExchangeRateRow {
            id: new_id(),
            currency_code: "USD".into(),
            rate_date: date!(2026 - 11 - 05),
            rate: dec("32.150000"),
            created_at: datetime!(2026-11-17 00:00 UTC),
            updated_at: datetime!(2026-11-17 00:00 UTC),
            version: 1,
        };
        assert_eq!(
            audit_json(&row),
            json!({ "currency_code": "USD", "rate_date": "2026-11-05", "rate": "32.150000" })
        );
        let rate = to_exchange_rate(row.clone());
        assert_eq!(rate.id, row.id);
        assert_eq!(rate.rate, row.rate);
    }

    #[test]
    fn error_messages() {
        assert_eq!(field_of(&currency_not_found("")), field::CURRENCY_CODE);
        assert!(currency_not_found("XYZ").to_string().contains("XYZ"));
        assert_eq!(not_found().code(), ErrorCode::NotFound);
    }
}

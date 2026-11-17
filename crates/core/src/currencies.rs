//! 幣別（docs/architecture.md §6.1、§8.6 `currencies`、§9.4）。
//!
//! - [`list_currencies`]：需要登入；預設只列出啟用中的
//! - [`create_currency`]、[`update_currency`]：需要 `settings.manage`，在單一 transaction 內完成變更與操作紀錄
//!
//! 幣別不可刪除，只能停用；本位幣（TWD）不可停用。`code`、`is_base` 不可修改。
//! 操作紀錄的 `entity_key` 為 [`ENTITY_KEY`]、`record_id` 為幣別的 `id`。

use hammer_db::currency::{self as db, CODE_UNIQUE, CurrencyRow, CurrencyUpdate, NewCurrency};
use hammer_db::sqlx::{PgPool, PgTransaction};
use hammer_db::{DbError, ensure_version_matched};
use hammer_shared::currency::{Currency, MAX_DECIMAL_PLACES, field};
use hammer_shared::{Id, audit as action, new_id, permission};
use serde_json::{Value, json};
use time::OffsetDateTime;

use crate::CoreError;
use crate::audit::{self, AuditDiff, AuditEntry};
use crate::auth::AuthContext;

/// 操作紀錄的 `entity_key`。
pub const ENTITY_KEY: &str = "currencies";

/// 幣別不存在時的錯誤訊息。
pub const CURRENCY_NOT_FOUND: &str = "找不到此幣別";

// ---------------------------------------------------------------------------
// 輸入與純邏輯
// ---------------------------------------------------------------------------

/// 新增幣別的輸入。
#[derive(Debug, Clone, Copy)]
pub struct NewCurrencyInput<'a> {
    pub code: &'a str,
    pub name: &'a str,
    pub symbol: Option<&'a str>,
    pub decimal_places: i16,
}

/// 修改幣別的輸入。`None` 的欄位不修改；`symbol` 為空字串時清除。
#[derive(Debug, Clone, Copy)]
pub struct CurrencyChanges<'a> {
    /// 讀取時取得的版本號（§9.4）。
    pub version: i32,
    pub name: Option<&'a str>,
    pub symbol: Option<&'a str>,
    pub decimal_places: Option<i16>,
    pub is_active: Option<bool>,
}

/// 可以修改的欄位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditableFields {
    pub name: String,
    pub symbol: Option<String>,
    pub decimal_places: i16,
    pub is_active: bool,
}

impl EditableFields {
    fn of(row: &CurrencyRow) -> Self {
        Self {
            name: row.name.clone(),
            symbol: row.symbol.clone(),
            decimal_places: row.decimal_places,
            is_active: row.is_active,
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "symbol": self.symbol,
            "decimal_places": self.decimal_places,
            "is_active": self.is_active,
        })
    }
}

/// 整理幣別代碼：去除前後空白、轉成大寫。用於查詢（不檢查格式）。
pub fn normalize_code(code: &str) -> String {
    code.trim().to_ascii_uppercase()
}

/// 檢查並整理新增的代碼：轉成大寫後必須是 3 個英文字母。
pub fn validate_code(code: &str) -> Result<String, CoreError> {
    let code = normalize_code(code);
    if code.len() == 3 && code.bytes().all(|b| b.is_ascii_uppercase()) {
        Ok(code)
    } else {
        Err(invalid(field::CODE, "幣別代碼必須是 3 個英文字母"))
    }
}

fn validate_name(name: &str) -> Result<&str, CoreError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid(field::NAME, "請輸入幣別名稱"));
    }
    Ok(name)
}

/// 符號去除前後空白；空字串視為沒有符號。
fn normalize_symbol(symbol: Option<&str>) -> Option<&str> {
    symbol.map(str::trim).filter(|s| !s.is_empty())
}

fn validate_decimal_places(places: i16) -> Result<i16, CoreError> {
    if (0..=MAX_DECIMAL_PLACES).contains(&places) {
        Ok(places)
    } else {
        Err(invalid(
            field::DECIMAL_PLACES,
            &format!("小數位數必須介於 0 到 {MAX_DECIMAL_PLACES}"),
        ))
    }
}

impl NewCurrencyInput<'_> {
    /// 檢查輸入，回傳整理後的 `(code, name, symbol, decimal_places)`。
    fn validate(&self) -> Result<(String, &str, Option<&str>, i16), CoreError> {
        Ok((
            validate_code(self.code)?,
            validate_name(self.name)?,
            normalize_symbol(self.symbol),
            validate_decimal_places(self.decimal_places)?,
        ))
    }
}

/// 檢查修改內容並與目前的值合併。沒有任何欄位改變時回傳 `None`。
///
/// 本位幣不可停用 → [`CoreError::InvalidField`]（`is_active`）。
pub fn plan_update(
    current: &EditableFields,
    is_base: bool,
    changes: &CurrencyChanges<'_>,
) -> Result<Option<EditableFields>, CoreError> {
    let name = match changes.name {
        Some(name) => validate_name(name)?.to_owned(),
        None => current.name.clone(),
    };
    let symbol = match changes.symbol {
        Some(symbol) => normalize_symbol(Some(symbol)).map(str::to_owned),
        None => current.symbol.clone(),
    };
    let decimal_places = match changes.decimal_places {
        Some(places) => validate_decimal_places(places)?,
        None => current.decimal_places,
    };
    let is_active = changes.is_active.unwrap_or(current.is_active);
    if is_base && !is_active {
        return Err(invalid(field::IS_ACTIVE, "本位幣不可停用"));
    }
    let next = EditableFields {
        name,
        symbol,
        decimal_places,
        is_active,
    };
    Ok((next != *current).then_some(next))
}

/// 資料列轉成 API 回應。
pub fn to_currency(row: CurrencyRow) -> Currency {
    Currency {
        id: row.id,
        code: row.code,
        name: row.name,
        symbol: row.symbol,
        decimal_places: row.decimal_places,
        is_base: row.is_base,
        is_active: row.is_active,
        created_at: row.created_at,
        updated_at: row.updated_at,
        version: row.version,
    }
}

fn invalid(field: &'static str, message: &str) -> CoreError {
    CoreError::InvalidField {
        field,
        message: message.to_owned(),
    }
}

fn not_found() -> CoreError {
    CoreError::NotFound(CURRENCY_NOT_FOUND.into())
}

// ---------------------------------------------------------------------------
// 查詢與寫入
// ---------------------------------------------------------------------------

/// 列出幣別（本位幣優先、再依代碼排序）。需要登入，不需要特定權限。
pub async fn list_currencies(
    pool: &PgPool,
    _ctx: &AuthContext,
    include_inactive: bool,
) -> Result<Vec<Currency>, CoreError> {
    let rows = db::list_currencies(pool, include_inactive).await?;
    Ok(rows.into_iter().map(to_currency).collect())
}

/// 新增幣別（啟用中、非本位幣）。需要 `settings.manage`。
///
/// 代碼格式不正確或與既有幣別（含已停用）重複 → [`CoreError::InvalidField`]（`code`）。
pub async fn create_currency(
    pool: &PgPool,
    ctx: &AuthContext,
    input: &NewCurrencyInput<'_>,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<Currency, CoreError> {
    ctx.require(permission::SETTINGS_MANAGE)?;
    let (code, name, symbol, decimal_places) = input.validate()?;

    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let id = new_id();
    db::insert_currency(
        &mut *tx,
        &NewCurrency {
            id,
            code: &code,
            name,
            symbol,
            decimal_places,
            created_by: ctx.user_id,
        },
    )
    .await
    .map_err(|e| {
        if e.unique_violation() == Some(CODE_UNIQUE) {
            invalid(field::CODE, "此幣別代碼已存在")
        } else {
            e.into()
        }
    })?;
    let row = fetch(&mut tx, &code).await?;
    let mut after = EditableFields::of(&row).to_json();
    after["code"] = row.code.clone().into();
    after["is_base"] = row.is_base.into();
    record(
        &mut tx,
        ctx,
        action::CREATE,
        id,
        AuditDiff::between(None, Some(&after))?,
        client_info,
        now,
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;

    tracing::info!(actor = %ctx.user_id, code = %code, "已新增幣別");
    Ok(to_currency(row))
}

/// 修改幣別的名稱、符號、小數位數、是否停用。需要 `settings.manage`。
///
/// - 不存在 → [`CoreError::NotFound`]
/// - `version` 不符 → `Conflict`（§9.4）
/// - 停用本位幣 → [`CoreError::InvalidField`]（`is_active`）
/// - 沒有任何欄位改變時不寫入，回傳目前的資料（`version` 不變）
pub async fn update_currency(
    pool: &PgPool,
    ctx: &AuthContext,
    code: &str,
    changes: &CurrencyChanges<'_>,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<Currency, CoreError> {
    ctx.require(permission::SETTINGS_MANAGE)?;
    let code = normalize_code(code);

    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let current = db::find_currency_for_update(&mut *tx, &code)
        .await?
        .ok_or_else(not_found)?;
    if current.version != changes.version {
        return Err(DbError::Conflict.into());
    }
    let before = EditableFields::of(&current);
    let Some(after) = plan_update(&before, current.is_base, changes)? else {
        return Ok(to_currency(current));
    };

    ensure_version_matched(
        db::update_currency(
            &mut *tx,
            &CurrencyUpdate {
                id: current.id,
                version: changes.version,
                name: &after.name,
                symbol: after.symbol.as_deref(),
                decimal_places: after.decimal_places,
                is_active: after.is_active,
                updated_by: ctx.user_id,
            },
        )
        .await?,
    )?;
    record(
        &mut tx,
        ctx,
        action::UPDATE,
        current.id,
        AuditDiff::between(Some(&before.to_json()), Some(&after.to_json()))?,
        client_info,
        now,
    )
    .await?;
    let row = fetch(&mut tx, &code).await?;
    tx.commit().await.map_err(DbError::from)?;

    tracing::info!(actor = %ctx.user_id, code = %code, "已修改幣別");
    Ok(to_currency(row))
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

async fn fetch(tx: &mut PgTransaction<'_>, code: &str) -> Result<CurrencyRow, CoreError> {
    db::find_currency(&mut **tx, code)
        .await?
        .ok_or_else(|| CoreError::Internal("寫入後找不到幣別".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hammer_shared::ErrorCode;

    fn field_of(err: &CoreError) -> &'static str {
        match err {
            CoreError::InvalidField { field, .. } => field,
            other => panic!("不是欄位錯誤：{other:?}"),
        }
    }

    fn current() -> EditableFields {
        EditableFields {
            name: "美元".into(),
            symbol: Some("US$".into()),
            decimal_places: 2,
            is_active: true,
        }
    }

    fn changes() -> CurrencyChanges<'static> {
        CurrencyChanges {
            version: 1,
            name: None,
            symbol: None,
            decimal_places: None,
            is_active: None,
        }
    }

    #[test]
    fn code_is_uppercased() {
        assert_eq!(validate_code(" gbp ").unwrap(), "GBP");
        assert_eq!(validate_code("Usd").unwrap(), "USD");
    }

    #[test]
    fn invalid_codes_are_rejected() {
        for code in ["", "US", "USDT", "U$D", "12A", "ÄBC", "臺幣幣"] {
            let err = validate_code(code).unwrap_err();
            assert_eq!(err.code(), ErrorCode::Validation);
            assert_eq!(field_of(&err), field::CODE, "{code}");
        }
    }

    #[test]
    fn new_input_is_validated() {
        let input = NewCurrencyInput {
            code: "gbp",
            name: " 英鎊 ",
            symbol: Some("  "),
            decimal_places: 2,
        };
        assert_eq!(input.validate().unwrap(), ("GBP".into(), "英鎊", None, 2));
        let err = NewCurrencyInput { name: " ", ..input }
            .validate()
            .unwrap_err();
        assert_eq!(field_of(&err), field::NAME);
        for places in [-1, 5] {
            let err = NewCurrencyInput {
                decimal_places: places,
                ..input
            }
            .validate()
            .unwrap_err();
            assert_eq!(field_of(&err), field::DECIMAL_PLACES);
        }
    }

    #[test]
    fn plan_merges_changes() {
        let next = plan_update(
            &current(),
            false,
            &CurrencyChanges {
                name: Some(" 美金 "),
                symbol: Some(""),
                decimal_places: Some(0),
                is_active: Some(false),
                ..changes()
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            next,
            EditableFields {
                name: "美金".into(),
                symbol: None,
                decimal_places: 0,
                is_active: false,
            }
        );
    }

    #[test]
    fn plan_without_changes_is_none() {
        assert_eq!(plan_update(&current(), false, &changes()).unwrap(), None);
        let same = CurrencyChanges {
            name: Some("美元 "),
            symbol: Some(" US$"),
            decimal_places: Some(2),
            is_active: Some(true),
            ..changes()
        };
        assert_eq!(plan_update(&current(), false, &same).unwrap(), None);
    }

    #[test]
    fn base_currency_cannot_be_deactivated() {
        let deactivate = CurrencyChanges {
            is_active: Some(false),
            ..changes()
        };
        let err = plan_update(&current(), true, &deactivate).unwrap_err();
        assert_eq!(field_of(&err), field::IS_ACTIVE);
        // 本位幣仍可修改名稱
        let rename = CurrencyChanges {
            name: Some("臺幣"),
            ..changes()
        };
        assert!(plan_update(&current(), true, &rename).unwrap().is_some());
    }

    #[test]
    fn plan_rejects_invalid_values() {
        let err = plan_update(
            &current(),
            false,
            &CurrencyChanges {
                name: Some(""),
                ..changes()
            },
        )
        .unwrap_err();
        assert_eq!(field_of(&err), field::NAME);
        let err = plan_update(
            &current(),
            false,
            &CurrencyChanges {
                decimal_places: Some(7),
                ..changes()
            },
        )
        .unwrap_err();
        assert_eq!(field_of(&err), field::DECIMAL_PLACES);
    }

    #[test]
    fn not_found_message() {
        assert_eq!(not_found().code(), ErrorCode::NotFound);
        assert_eq!(not_found().to_string(), CURRENCY_NOT_FOUND);
    }
}

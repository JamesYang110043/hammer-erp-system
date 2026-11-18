//! 採購單（之後也用於進貨單）存檔時由伺服器處理的部分（docs/architecture.md §8.8「幣別與匯率」
//! 「金額與稅額」「存檔時的檢查」）。由 [`crate::records`] 在新增、修改時依序呼叫：
//!
//! 1. [`prefill`]（只有新增，欄位驗證之前）：沒有帶 `currency_code` 時帶入廠商的預設幣別；沒有帶
//!    `tax_type`、欄位定義也沒有 `default_value` 時，本位幣為 `taxable`、外幣為 `zero_rated`
//! 2. [`check_line_data`]（每一列明細，欄位驗證之後）：`qty > 0`、`unit_price ≥ 0`
//! 3. [`derive_currency`]（主表欄位驗證之後）：幣別必須存在且啟用中（只在新增或值有變動時檢查）；
//!    本位幣的匯率一律 `1.000000`；新增、或修改時幣別有變動但沒有帶 `exchange_rate` 時，帶入單據日期的
//!    適用匯率（找不到時留空）；匯率必須大於 0
//! 4. [`apply_amounts`]（全部驗證通過之後）：依明細重新計算 `amount`、`subtotal`、`tax_amount`、
//!    `total`、`total_twd`（[`super::amounts`]）
//!
//! 修改時 [`crate::records`] 一律帶入所有明細（[`recalculates_lines`]），所以只改表頭（例如幣別、匯率）
//! 時也會重新計算明細金額。

use std::str::FromStr;

use hammer_db::currency::{self as currency_db, CurrencyRow};
use hammer_db::exchange_rate as rate_db;
use hammer_db::purchasing as db;
use hammer_db::sqlx::PgConnection;
use hammer_shared::Id;
use hammer_shared::decimal::Decimal;
use hammer_shared::schema::{DataType, FieldDefinition, FieldValue, parse_value};
use serde_json::{Map, Value};
use time::Date;

use super::PURCHASE_ORDER;
use super::amounts::{self, AMOUNT_SCALE, RATE_SCALE, TWD_SCALE};
use crate::CoreError;
use crate::record_data::FieldError;

/// 採購明細的明細子表代碼。
pub const PURCHASE_ORDER_LINE: &str = "purchase_order_line";

pub const MSG_CURRENCY_INVALID: &str = "幣別不存在或已停用";
pub const MSG_RATE_POSITIVE: &str = "匯率必須大於 0";
pub const MSG_RATE_MISSING: &str = "找不到匯率，請輸入匯率";
pub const MSG_QTY_POSITIVE: &str = "數量必須大於 0";
pub const MSG_PRICE_NEGATIVE: &str = "單價不可小於 0";

pub(crate) const SUPPLIER_ID: &str = "supplier_id";
pub(crate) const CURRENCY_CODE: &str = "currency_code";
pub(crate) const EXCHANGE_RATE: &str = "exchange_rate";
pub(crate) const TAX_TYPE: &str = "tax_type";
const SUBTOTAL: &str = "subtotal";
const TAX_AMOUNT: &str = "tax_amount";
const TOTAL: &str = "total";
const TOTAL_TWD: &str = "total_twd";
pub(crate) const QTY: &str = "qty";
pub(crate) const UNIT_PRICE: &str = "unit_price";
const AMOUNT: &str = "amount";

/// 有幣別、匯率與金額計算的單據。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PricedDocument {
    /// 單據的資料類別代碼。
    pub entity_key: &'static str,
    /// 明細子表代碼。
    pub line_key: &'static str,
    /// 匯率的適用日期欄位（單據日期）。
    pub date_field: &'static str,
}

const PRICED: &[PricedDocument] = &[PricedDocument {
    entity_key: PURCHASE_ORDER,
    line_key: PURCHASE_ORDER_LINE,
    date_field: "order_date",
}];

/// `entity_key` 是否為有幣別、匯率與金額計算的單據。
pub fn priced(entity_key: &str) -> Option<&'static PricedDocument> {
    PRICED.iter().find(|d| d.entity_key == entity_key)
}

/// 修改時是否要帶入所有明細（重新計算明細金額）。
pub fn recalculates_lines(entity_key: &str) -> bool {
    priced(entity_key).is_some()
}

/// 明細子表 `detail_key` 是否為有金額計算的單據明細。
fn priced_line(detail_key: &str) -> bool {
    PRICED.iter().any(|d| d.line_key == detail_key)
}

fn is_missing(input: &Map<String, Value>, key: &str) -> bool {
    input.get(key).is_none_or(Value::is_null)
}

pub(crate) fn decimal_of(v: Option<&Value>) -> Option<Decimal> {
    match v? {
        Value::String(s) => Decimal::from_str(s.trim()).ok(),
        _ => None,
    }
}

pub(crate) fn id_of(v: Option<&Value>) -> Option<Id> {
    match parse_value(DataType::Relation, v?) {
        Ok(Some(FieldValue::Relation(id))) => Some(id),
        _ => None,
    }
}

fn date_of(v: Option<&Value>) -> Option<Date> {
    match parse_value(DataType::Date, v?) {
        Ok(Some(FieldValue::Date(d))) => Some(d),
        _ => None,
    }
}

fn string(v: Decimal) -> Value {
    Value::String(v.to_string())
}

/// 新增時，在欄位驗證之前帶入預設的幣別與稅別（`input` 為 request 的 `data`）。
pub(crate) async fn prefill(
    conn: &mut PgConnection,
    entity_key: &str,
    fields: &[FieldDefinition],
    input: &mut Map<String, Value>,
) -> Result<(), CoreError> {
    if priced(entity_key).is_none() {
        return Ok(());
    }
    if is_missing(input, CURRENCY_CODE)
        && let Some(supplier_id) = id_of(input.get(SUPPLIER_ID))
        && let Some(code) = db::supplier_default_currency(&mut *conn, supplier_id).await?
    {
        input.insert(CURRENCY_CODE.into(), Value::String(code));
    }
    let has_default = fields
        .iter()
        .any(|f| f.field_key == TAX_TYPE && f.default_value.as_ref().is_some_and(|v| !v.is_null()));
    if is_missing(input, TAX_TYPE) && !has_default {
        let is_base = match input.get(CURRENCY_CODE).and_then(Value::as_str) {
            Some(code) => currency_db::find_currency(&mut *conn, code)
                .await?
                .is_none_or(|c| c.is_base),
            None => true,
        };
        input.insert(
            TAX_TYPE.into(),
            Value::String(amounts::default_tax_type(is_base).into()),
        );
    }
    Ok(())
}

/// 一列明細本身的檢查（欄位驗證之後）：`qty > 0`、`unit_price ≥ 0`。不是有金額計算的明細時為空。
pub fn check_line_data(detail_key: &str, data: &Map<String, Value>) -> Vec<FieldError> {
    if !priced_line(detail_key) {
        return Vec::new();
    }
    line_errors(decimal_of(data.get(QTY)), decimal_of(data.get(UNIT_PRICE)))
}

/// `qty > 0`、`unit_price ≥ 0`；沒有值的不檢查（必填由欄位驗證處理）。
pub fn line_errors(qty: Option<Decimal>, unit_price: Option<Decimal>) -> Vec<FieldError> {
    let mut errors = Vec::new();
    if qty.is_some_and(|q| q <= Decimal::ZERO) {
        errors.push(FieldError {
            field_key: QTY.into(),
            message: MSG_QTY_POSITIVE.into(),
        });
    }
    if unit_price.is_some_and(|p| p < Decimal::ZERO) {
        errors.push(FieldError {
            field_key: UNIT_PRICE.into(),
            message: MSG_PRICE_NEGATIVE.into(),
        });
    }
    errors
}

/// 幣別與匯率（主表欄位驗證之後）。`rate_given`：request 的 `data` 是否帶了 `exchange_rate`。
pub(crate) async fn derive_currency(
    conn: &mut PgConnection,
    entity_key: &str,
    master: Result<Map<String, Value>, Vec<FieldError>>,
    existing: Option<&Map<String, Value>>,
    rate_given: bool,
) -> Result<Result<Map<String, Value>, Vec<FieldError>>, CoreError> {
    let Some(doc) = priced(entity_key) else {
        return Ok(master);
    };
    let Ok(mut data) = master else {
        return Ok(master);
    };
    let Some(code) = data
        .get(CURRENCY_CODE)
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return Ok(Ok(data));
    };
    let changed = existing.is_none_or(|e| e.get(CURRENCY_CODE) != data.get(CURRENCY_CODE));
    let currency = currency_db::find_currency(&mut *conn, &code).await?;
    let currency = match currency {
        Some(c) if c.is_active || !changed => c,
        _ => {
            return Ok(Err(vec![FieldError {
                field_key: CURRENCY_CODE.into(),
                message: MSG_CURRENCY_INVALID.into(),
            }]));
        }
    };
    let rate = match plan_rate(&currency, changed, rate_given) {
        RatePlan::Base => Some(crate::exchange_rates::base_rate()),
        RatePlan::Effective => match date_of(data.get(doc.date_field)) {
            Some(date) => rate_db::find_effective_rate(&mut *conn, &code, date)
                .await?
                .map(|(_, rate)| amounts::with_scale(rate, RATE_SCALE)),
            None => None,
        },
        RatePlan::Keep => {
            let rate = decimal_of(data.get(EXCHANGE_RATE));
            if rate.is_some_and(|r| r <= Decimal::ZERO) {
                return Ok(Err(vec![FieldError {
                    field_key: EXCHANGE_RATE.into(),
                    message: MSG_RATE_POSITIVE.into(),
                }]));
            }
            rate
        }
    };
    match rate {
        Some(rate) => data.insert(EXCHANGE_RATE.into(), string(rate)),
        None => data.remove(EXCHANGE_RATE),
    };
    Ok(Ok(data))
}

/// 匯率的來源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RatePlan {
    /// 本位幣：一律 1。
    Base,
    /// 帶入單據日期的適用匯率。
    Effective,
    /// 使用 request（或既有資料）的值。
    Keep,
}

/// 新增（`changed = true`）或幣別有變動、且沒有帶匯率時帶入適用匯率；只有日期變動時不更新。
fn plan_rate(currency: &CurrencyRow, changed: bool, rate_given: bool) -> RatePlan {
    if currency.is_base {
        RatePlan::Base
    } else if changed && !rate_given {
        RatePlan::Effective
    } else {
        RatePlan::Keep
    }
}

/// 依明細重新計算金額並寫入 `data` 與每一列明細（全部驗證通過之後）。`lines` 為所有明細的 `data`。
pub(crate) async fn apply_amounts(
    conn: &mut PgConnection,
    entity_key: &str,
    data: &mut Map<String, Value>,
    lines: Vec<&mut Map<String, Value>>,
) -> Result<(), CoreError> {
    if priced(entity_key).is_none() {
        return Ok(());
    }
    let code = data
        .get(CURRENCY_CODE)
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::Internal("單據沒有幣別".into()))?;
    let currency = currency_db::find_currency(&mut *conn, code)
        .await?
        .ok_or_else(|| CoreError::Internal(format!("找不到幣別 {code}")))?;
    let places = u32::try_from(currency.decimal_places)
        .map_err(|_| CoreError::Internal(format!("幣別 {code} 的小數位數不正確")))?;
    let tax_type = data
        .get(TAX_TYPE)
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::Internal("單據沒有稅別".into()))?;
    let tax_rate = db::find_tax_rate(&mut *conn, tax_type)
        .await?
        .ok_or_else(|| CoreError::Internal(format!("找不到稅別 {tax_type}")))?;

    let mut line_amounts = Vec::with_capacity(lines.len());
    for line in lines {
        let (Some(qty), Some(price)) =
            (decimal_of(line.get(QTY)), decimal_of(line.get(UNIT_PRICE)))
        else {
            return Err(CoreError::Internal("明細沒有數量或單價".into()));
        };
        let amount = amounts::line_amount(qty, price, places);
        line.insert(AMOUNT.into(), string(amount));
        line_amounts.push(amount);
    }
    let totals = amounts::totals(
        line_amounts,
        tax_rate,
        places,
        decimal_of(data.get(EXCHANGE_RATE)),
    );
    write_totals(data, &totals);
    Ok(())
}

fn write_totals(data: &mut Map<String, Value>, totals: &amounts::Totals) {
    debug_assert_eq!(totals.subtotal.scale(), AMOUNT_SCALE);
    data.insert(SUBTOTAL.into(), string(totals.subtotal));
    data.insert(TAX_AMOUNT.into(), string(totals.tax_amount));
    data.insert(TOTAL.into(), string(totals.total));
    match totals.total_twd {
        Some(twd) => {
            debug_assert_eq!(twd.scale(), TWD_SCALE);
            data.insert(TOTAL_TWD.into(), string(twd))
        }
        None => data.remove(TOTAL_TWD),
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use time::OffsetDateTime;
    use uuid::Uuid;

    fn currency(code: &str, is_base: bool) -> CurrencyRow {
        CurrencyRow {
            id: Uuid::nil(),
            code: code.into(),
            name: code.into(),
            symbol: None,
            decimal_places: if is_base { 0 } else { 2 },
            is_base,
            is_active: true,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            version: 1,
        }
    }

    #[test]
    fn rate_plan_follows_currency_rules() {
        let twd = currency("TWD", true);
        let usd = currency("USD", false);
        // 本位幣一律 1，request 帶了其他值也覆蓋
        assert_eq!(plan_rate(&twd, true, true), RatePlan::Base);
        assert_eq!(plan_rate(&twd, false, false), RatePlan::Base);
        // 新增或改幣別、沒有帶匯率 → 帶入適用匯率
        assert_eq!(plan_rate(&usd, true, false), RatePlan::Effective);
        // 使用者有輸入，或幣別沒有變動（只改日期）→ 不自動更新
        assert_eq!(plan_rate(&usd, true, true), RatePlan::Keep);
        assert_eq!(plan_rate(&usd, false, false), RatePlan::Keep);
    }

    #[test]
    fn line_checks_qty_and_price() {
        let d = |s: &str| Some(Decimal::from_str(s).unwrap());
        assert!(line_errors(d("1"), d("0")).is_empty());
        let keys = |e: Vec<FieldError>| e.into_iter().map(|e| e.message).collect::<Vec<_>>();
        assert_eq!(keys(line_errors(d("0"), d("1"))), [MSG_QTY_POSITIVE]);
        assert_eq!(
            keys(line_errors(d("-1"), d("-0.01"))),
            [MSG_QTY_POSITIVE, MSG_PRICE_NEGATIVE]
        );
        assert!(line_errors(None, None).is_empty());
    }

    #[test]
    fn line_data_checked_only_for_priced_lines() {
        let data = json!({ "qty": "0.0000", "unit_price": "1.0000" });
        let data = data.as_object().unwrap();
        assert_eq!(check_line_data(PURCHASE_ORDER_LINE, data).len(), 1);
        assert!(check_line_data("supplier_contact", data).is_empty());
    }

    #[test]
    fn totals_are_written_as_padded_strings() {
        let mut data = json!({ "total_twd": "1.00" }).as_object().unwrap().clone();
        let t = amounts::totals(
            [Decimal::from_str("123.4500").unwrap()],
            Decimal::ZERO,
            2,
            None,
        );
        write_totals(&mut data, &t);
        assert_eq!(
            Value::Object(data),
            json!({ "subtotal": "123.4500", "tax_amount": "0.0000", "total": "123.4500" })
        );
    }

    #[test]
    fn only_purchase_orders_are_priced() {
        assert!(priced(PURCHASE_ORDER).is_some());
        assert!(recalculates_lines(PURCHASE_ORDER));
        assert!(!recalculates_lines("stock_adjustment"));
        assert!(priced_line(PURCHASE_ORDER_LINE));
    }
}

//! 銷貨單據存檔時由伺服器處理的部分（docs/architecture.md §8.9「金額與稅額」「存檔時的檢查」）。由
//! [`crate::records`] 在新增、修改時依序呼叫：
//!
//! 1. [`prefill`]（只有新增，欄位驗證之前）：沒有帶 `tax_type`、欄位定義也沒有 `default_value` 時，
//!    出貨單有 `sales_order_id` → 帶入訂單的稅別；否則明細商品的稅別全部相同 → 帶入該值，否則 `taxable`
//! 2. [`line_product_names`]（訂單、出貨明細，欄位驗證之前）：沒帶品名（或空白）、換了商品時，依
//!    訂單明細的品名 → 客戶品名 → 商品的 `name` 帶入，見該函式
//! 3. [`check_line_data`]（每一列明細，欄位驗證之後）：`qty > 0`、`unit_price ≥ 0`、
//!    `0 ≤ discount_rate ≤ 1`；出貨明細的批號去除前後空白（空字串視為留空）、不可有控制字元
//! 4. [`apply_amounts`]（全部驗證通過之後）：依明細重新計算 `amount`、`subtotal`、`tax_amount`、
//!    `total`（[`super::amounts`]）
//!
//! 修改時 [`crate::records`] 一律帶入所有明細（[`recalculates_lines`]），所以只改表頭（例如稅別、含稅）
//! 時也會重新計算明細金額。
//!
//! 訂單、出貨單、銷貨退回都有金額計算；銷貨退回的稅別、含稅、單價、折扣率由出貨單帶入
//! （[`super::returns::derive_return`]），不帶入預設稅別。

use std::collections::{BTreeMap, HashMap};

use hammer_db::currency as currency_db;
use hammer_db::purchasing as purchasing_db;
use hammer_db::sales as db;
use hammer_db::sqlx::PgConnection;
use hammer_shared::Id;
use hammer_shared::decimal::Decimal;
use hammer_shared::record::LineInput;
use hammer_shared::schema::FieldDefinition;
use serde_json::{Map, Value};

use super::amounts;
use super::{
    SALES_ORDER, SALES_ORDER_LINE, SALES_RETURN, SALES_RETURN_LINE, SHIPMENT, SHIPMENT_LINE,
};
use crate::CoreError;
use crate::inventory::normalize_lot_no;
use crate::purchasing::amounts::AMOUNT_SCALE;
use crate::purchasing::save::{decimal_of, id_of, line_errors as qty_price_errors};
use crate::record_data::FieldError;

/// 折扣率超出範圍。
pub const MSG_DISCOUNT_RANGE: &str = "折扣率必須介於 0 與 1 之間";
/// 品名去除前後空白後為空。
pub const MSG_PRODUCT_NAME_REQUIRED: &str = "此欄位為必填";
/// 品名有控制字元。
pub const MSG_PRODUCT_NAME_CONTROL: &str = "品名不可包含控制字元";

/// 銷貨的幣別（台幣）：金額的小數位數取這個幣別的 `decimal_places`。
pub const SALES_CURRENCY: &str = "TWD";
/// 找不到台幣時的小數位數。
const DEFAULT_PLACES: u32 = 0;

pub(crate) const CUSTOMER_ID: &str = "customer_id";
pub(crate) const TAX_TYPE: &str = "tax_type";
pub(crate) const TAX_INCLUDED: &str = "tax_included";
pub(crate) const SALES_ORDER_ID: &str = "sales_order_id";
pub(crate) const LOT_NO: &str = "lot_no";
const PRODUCT_ID: &str = "product_id";
const SALES_ORDER_LINE_ID: &str = "sales_order_line_id";
pub(crate) const PRODUCT_NAME: &str = "product_name";
const QTY: &str = "qty";
const UNIT_PRICE: &str = "unit_price";
const DISCOUNT_RATE: &str = "discount_rate";
const AMOUNT: &str = "amount";
const SUBTOTAL: &str = "subtotal";
const TAX_AMOUNT: &str = "tax_amount";
const TOTAL: &str = "total";

/// 有金額計算的銷貨單據。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SalesDocument {
    /// 單據的資料類別代碼。
    pub entity_key: &'static str,
    /// 明細子表代碼。
    pub line_key: &'static str,
    /// 新增時是否由 [`prefill`] 帶入預設稅別（銷貨退回沿用出貨單的值，不帶入）。
    pub prefills_tax: bool,
}

const PRICED: &[SalesDocument] = &[
    SalesDocument {
        entity_key: SALES_ORDER,
        line_key: SALES_ORDER_LINE,
        prefills_tax: true,
    },
    SalesDocument {
        entity_key: SHIPMENT,
        line_key: SHIPMENT_LINE,
        prefills_tax: true,
    },
    SalesDocument {
        entity_key: SALES_RETURN,
        line_key: SALES_RETURN_LINE,
        prefills_tax: false,
    },
];

/// `entity_key` 是否為有金額計算的銷貨單據。
pub fn with_amounts(entity_key: &str) -> Option<&'static SalesDocument> {
    PRICED.iter().find(|d| d.entity_key == entity_key)
}

/// 修改時是否要帶入所有明細（重新計算明細金額）。
pub fn recalculates_lines(entity_key: &str) -> bool {
    with_amounts(entity_key).is_some()
}

fn priced_line(detail_key: &str) -> bool {
    PRICED.iter().any(|d| d.line_key == detail_key)
}

fn string(v: Decimal) -> Value {
    Value::String(v.to_string())
}

/// 新增時，在欄位驗證之前帶入預設的稅別（`input` 為 request 的 `data`，`lines` 為 request 的明細）。
pub(crate) async fn prefill(
    conn: &mut PgConnection,
    entity_key: &str,
    fields: &[FieldDefinition],
    input: &mut Map<String, Value>,
    lines: &BTreeMap<String, Vec<LineInput>>,
) -> Result<(), CoreError> {
    let Some(doc) = with_amounts(entity_key).filter(|d| d.prefills_tax) else {
        return Ok(());
    };
    let missing = input.get(TAX_TYPE).is_none_or(Value::is_null);
    let has_default = fields
        .iter()
        .any(|f| f.field_key == TAX_TYPE && f.default_value.as_ref().is_some_and(|v| !v.is_null()));
    if !missing || has_default {
        return Ok(());
    }
    if entity_key == SHIPMENT
        && let Some(order_id) = id_of(input.get(SALES_ORDER_ID))
        && let Some(order) = db::find_order_for_shipment(&mut *conn, order_id).await?
    {
        input.insert(TAX_TYPE.into(), Value::String(order.tax_type));
        return Ok(());
    }
    let mut ids: Vec<_> = lines
        .get(doc.line_key)
        .into_iter()
        .flatten()
        .filter_map(|line| id_of(line.data.get(PRODUCT_ID)))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    let types = if ids.is_empty() {
        Vec::new()
    } else {
        db::product_tax_types(&mut *conn, &ids).await?
    };
    let tax_type = amounts::default_tax_type(types.iter().map(String::as_str));
    input.insert(TAX_TYPE.into(), Value::String(tax_type.into()));
    Ok(())
}

/// 明細 request 的一列 `data` 與修改前的資料（新增的列為 `None`）。
pub(crate) type LineWithExisting<'a> = (&'a Map<String, Value>, Option<&'a Map<String, Value>>);

/// 訂單、出貨明細沒帶品名時要帶入的品名（欄位驗證之前；docs/architecture.md §8.9「明細的品名」）。
///
/// `rows` 為同一個明細子表 request 中的每一列 `(data, 修改前的資料)`（新增的列為 `None`），
/// `customer_id` 為表頭的客戶（修改時已與既有資料合併）。回傳與 `rows` 同順序，需要帶入的列為
/// `Some(品名)`：
///
/// - 需要帶入：`product_name` 為 `null` 或空白；沒帶 `product_name` 時，新增的列或修改時換了
///   `product_id`。只改其他欄位時保留原本的品名
/// - 帶入順序：出貨明細有 `sales_order_line_id` → 訂單明細的品名；否則這位客戶未刪除的客戶品名 →
///   商品的 `name`
///
/// 所有列一起查詢（不是每列各查一次）。商品不存在、已刪除或格式錯誤時不帶入，交給欄位驗證回報。
/// 表頭換客戶時不重新帶入既有列的品名（由桌面程式詢問使用者後處理）。
pub(crate) async fn line_product_names(
    conn: &mut PgConnection,
    detail_key: &str,
    customer_id: Option<Id>,
    rows: &[LineWithExisting<'_>],
) -> Result<Vec<Option<String>>, CoreError> {
    let mut out = vec![None; rows.len()];
    if detail_key != SALES_ORDER_LINE && detail_key != SHIPMENT_LINE {
        return Ok(out);
    }
    let value = |input: &Map<String, Value>, existing: Option<&Map<String, Value>>, key: &str| {
        if input.contains_key(key) {
            id_of(input.get(key))
        } else {
            id_of(existing.and_then(|e| e.get(key)))
        }
    };
    // 需要帶入的列：(位置, 商品, 訂單明細)
    let wanted: Vec<(usize, Option<Id>, Option<Id>)> = rows
        .iter()
        .enumerate()
        .filter(|(_, (input, existing))| needs_product_name(input, *existing))
        .map(|(i, (input, existing))| {
            let order_line = if detail_key == SHIPMENT_LINE {
                value(input, *existing, SALES_ORDER_LINE_ID)
            } else {
                None
            };
            (i, value(input, *existing, PRODUCT_ID), order_line)
        })
        .collect();
    if wanted.is_empty() {
        return Ok(out);
    }

    let mut order_lines: Vec<Id> = wanted.iter().filter_map(|w| w.2).collect();
    order_lines.sort_unstable();
    order_lines.dedup();
    let order_names: HashMap<Id, String> = if order_lines.is_empty() {
        HashMap::new()
    } else {
        db::order_line_product_names(&mut *conn, &order_lines)
            .await?
            .into_iter()
            .collect()
    };

    let mut products: Vec<Id> = wanted.iter().filter_map(|w| w.1).collect();
    products.sort_unstable();
    products.dedup();
    let (customer_names, product_names): (HashMap<Id, String>, HashMap<Id, String>) =
        if products.is_empty() {
            (HashMap::new(), HashMap::new())
        } else {
            let customer_names = match customer_id {
                Some(customer) => db::customer_product_names(&mut *conn, customer, &products)
                    .await?
                    .into_iter()
                    .collect(),
                None => HashMap::new(),
            };
            let product_names = db::product_names(&mut *conn, &products)
                .await?
                .into_iter()
                .collect();
            (customer_names, product_names)
        };

    for (i, product, order_line) in wanted {
        out[i] = order_line
            .and_then(|l| order_names.get(&l))
            .or_else(|| product.and_then(|p| customer_names.get(&p)))
            .or_else(|| product.and_then(|p| product_names.get(&p)))
            .cloned();
    }
    Ok(out)
}

/// 是否需要帶入品名（見 [`line_product_names`]）。
fn needs_product_name(input: &Map<String, Value>, existing: Option<&Map<String, Value>>) -> bool {
    match input.get(PRODUCT_NAME) {
        Some(Value::Null) => true,
        Some(Value::String(s)) => s.trim().is_empty(),
        Some(_) => false,
        None => match existing {
            None => true,
            Some(existing) => input
                .get(PRODUCT_ID)
                .is_some_and(|p| id_of(Some(p)) != id_of(existing.get(PRODUCT_ID))),
        },
    }
}

/// 一列明細本身的檢查（欄位驗證之後）；訂單、出貨明細的品名去除前後空白後寫回、不可有控制字元；
/// 出貨明細的批號去除前後空白後寫回（空字串時移除）並檢查格式。
/// 不是銷貨單據的明細時為空。
pub fn check_line_data(detail_key: &str, data: &mut Map<String, Value>) -> Vec<FieldError> {
    if !priced_line(detail_key) {
        return Vec::new();
    }
    let mut errors = line_errors(
        decimal_of(data.get(QTY)),
        decimal_of(data.get(UNIT_PRICE)),
        decimal_of(data.get(DISCOUNT_RATE)),
    );
    if detail_key != SALES_RETURN_LINE {
        errors.extend(normalize_product_name(data));
    }
    if detail_key == SHIPMENT_LINE
        && let Some(Value::String(raw)) = data.get(LOT_NO)
    {
        match normalize_lot_no(raw) {
            Ok(Some(lot_no)) => {
                data.insert(LOT_NO.into(), Value::String(lot_no));
            }
            Ok(None) => {
                data.remove(LOT_NO);
            }
            Err(message) => errors.push(FieldError {
                field_key: LOT_NO.into(),
                message,
            }),
        }
    }
    errors
}

/// 訂單、出貨明細的品名去除前後空白後寫回；去除後為空 → 必填錯誤，有控制字元 → 錯誤。
/// 長度由欄位定義的 `max_length`（200）檢查。
fn normalize_product_name(data: &mut Map<String, Value>) -> Option<FieldError> {
    let Some(Value::String(raw)) = data.get(PRODUCT_NAME) else {
        return None;
    };
    let trimmed = raw.trim();
    let message = if trimmed.is_empty() {
        MSG_PRODUCT_NAME_REQUIRED
    } else if trimmed.chars().any(char::is_control) {
        MSG_PRODUCT_NAME_CONTROL
    } else {
        if trimmed.len() != raw.len() {
            let trimmed = trimmed.to_owned();
            data.insert(PRODUCT_NAME.into(), Value::String(trimmed));
        }
        return None;
    };
    data.remove(PRODUCT_NAME);
    Some(FieldError {
        field_key: PRODUCT_NAME.into(),
        message: message.into(),
    })
}

/// `qty > 0`、`unit_price ≥ 0`、`0 ≤ discount_rate ≤ 1`；沒有值的不檢查（必填由欄位驗證處理）。
pub fn line_errors(
    qty: Option<Decimal>,
    unit_price: Option<Decimal>,
    discount_rate: Option<Decimal>,
) -> Vec<FieldError> {
    let mut errors = qty_price_errors(qty, unit_price);
    if discount_rate.is_some_and(|r| r < Decimal::ZERO || r > Decimal::ONE) {
        errors.push(FieldError {
            field_key: DISCOUNT_RATE.into(),
            message: MSG_DISCOUNT_RANGE.into(),
        });
    }
    errors
}

/// 金額的小數位數：台幣在存檔當下的 `decimal_places`（找不到時為 0）。
pub(crate) async fn sales_places(conn: &mut PgConnection) -> Result<u32, CoreError> {
    match currency_db::find_currency(&mut *conn, SALES_CURRENCY).await? {
        Some(c) => u32::try_from(c.decimal_places)
            .map_err(|_| CoreError::Internal("台幣的小數位數不正確".into())),
        None => Ok(DEFAULT_PLACES),
    }
}

/// 依明細重新計算金額並寫入 `data` 與每一列明細（全部驗證通過之後）。`lines` 為所有明細的 `data`。
pub(crate) async fn apply_amounts(
    conn: &mut PgConnection,
    entity_key: &str,
    data: &mut Map<String, Value>,
    lines: Vec<&mut Map<String, Value>>,
) -> Result<(), CoreError> {
    if with_amounts(entity_key).is_none() {
        return Ok(());
    }
    let places = sales_places(&mut *conn).await?;
    let tax_type = data
        .get(TAX_TYPE)
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::Internal("單據沒有稅別".into()))?;
    let tax_rate = purchasing_db::find_tax_rate(&mut *conn, tax_type)
        .await?
        .ok_or_else(|| CoreError::Internal(format!("找不到稅別 {tax_type}")))?;
    let tax_included = data
        .get(TAX_INCLUDED)
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let mut line_amounts = Vec::with_capacity(lines.len());
    for line in lines {
        let (Some(qty), Some(price)) =
            (decimal_of(line.get(QTY)), decimal_of(line.get(UNIT_PRICE)))
        else {
            return Err(CoreError::Internal("明細沒有數量或單價".into()));
        };
        let discount = decimal_of(line.get(DISCOUNT_RATE)).unwrap_or(Decimal::ZERO);
        let amount = amounts::line_amount(qty, price, discount, places);
        line.insert(AMOUNT.into(), string(amount));
        line_amounts.push(amount);
    }
    let totals = amounts::totals(line_amounts, tax_rate, tax_included, places);
    write_totals(data, &totals);
    Ok(())
}

fn write_totals(data: &mut Map<String, Value>, totals: &amounts::Totals) {
    debug_assert_eq!(totals.subtotal.scale(), AMOUNT_SCALE);
    data.insert(SUBTOTAL.into(), string(totals.subtotal));
    data.insert(TAX_AMOUNT.into(), string(totals.tax_amount));
    data.insert(TOTAL.into(), string(totals.total));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::str::FromStr;

    fn messages(e: Vec<FieldError>) -> Vec<String> {
        e.into_iter().map(|e| e.message).collect()
    }

    #[test]
    fn product_name_is_needed_when_missing_blank_or_product_changed() {
        let p1 = "01a211f5-20e9-7b08-9d17-2a0dbce98c12";
        let p2 = "01a211f5-20ea-704c-9ef5-4cb1c369cd4d";
        let map = |v: Value| v.as_object().unwrap().clone();
        let existing = map(json!({ "product_id": p1, "product_name": "舊品名" }));

        // 新增：沒帶、null、空白 → 帶入；有值 → 不帶入
        assert!(needs_product_name(&map(json!({ "product_id": p1 })), None));
        assert!(needs_product_name(
            &map(json!({ "product_id": p1, "product_name": null })),
            None
        ));
        assert!(needs_product_name(
            &map(json!({ "product_id": p1, "product_name": "  " })),
            None
        ));
        assert!(!needs_product_name(
            &map(json!({ "product_id": p1, "product_name": "X 型扳手" })),
            None
        ));

        // 修改：只改其他欄位 → 保留；換商品 → 帶入；同一商品 → 保留；空白 → 帶入
        assert!(!needs_product_name(
            &map(json!({ "qty": "5" })),
            Some(&existing)
        ));
        assert!(needs_product_name(
            &map(json!({ "product_id": p2 })),
            Some(&existing)
        ));
        assert!(!needs_product_name(
            &map(json!({ "product_id": p1 })),
            Some(&existing)
        ));
        assert!(needs_product_name(
            &map(json!({ "product_name": "" })),
            Some(&existing)
        ));
        assert!(!needs_product_name(
            &map(json!({ "product_id": p2, "product_name": "新品名" })),
            Some(&existing)
        ));
    }

    #[test]
    fn product_name_is_trimmed_and_checked_on_order_and_shipment_lines() {
        for detail in [SALES_ORDER_LINE, SHIPMENT_LINE] {
            let mut data = json!({ "product_name": "  X 型扳手 " })
                .as_object()
                .unwrap()
                .clone();
            assert!(check_line_data(detail, &mut data).is_empty(), "{detail}");
            assert_eq!(data["product_name"], json!("X 型扳手"));

            let mut data = json!({ "product_name": "  " }).as_object().unwrap().clone();
            assert_eq!(
                messages(check_line_data(detail, &mut data)),
                [MSG_PRODUCT_NAME_REQUIRED]
            );
            let mut data = json!({ "product_name": "X\n型" })
                .as_object()
                .unwrap()
                .clone();
            assert_eq!(
                messages(check_line_data(detail, &mut data)),
                [MSG_PRODUCT_NAME_CONTROL]
            );
        }
        // 銷貨退回的品名是系統維護，不檢查
        let mut data = json!({ "product_name": " X " })
            .as_object()
            .unwrap()
            .clone();
        assert!(check_line_data(SALES_RETURN_LINE, &mut data).is_empty());
    }

    #[test]
    fn line_checks_qty_price_and_discount() {
        let d = |s: &str| Some(Decimal::from_str(s).unwrap());
        assert!(line_errors(d("1"), d("0"), d("0")).is_empty());
        assert!(line_errors(d("1"), d("0"), d("1")).is_empty());
        assert_eq!(
            messages(line_errors(d("0"), d("-1"), d("1.0001"))),
            [
                crate::purchasing::save::MSG_QTY_POSITIVE,
                crate::purchasing::save::MSG_PRICE_NEGATIVE,
                MSG_DISCOUNT_RANGE
            ]
        );
        let errors = line_errors(d("1"), d("1"), d("-0.1"));
        assert_eq!(errors[0].field_key, DISCOUNT_RATE);
        assert!(line_errors(None, None, None).is_empty());
    }

    #[test]
    fn line_data_checked_only_for_sales_lines() {
        let data = json!({ "qty": "0.0000", "unit_price": "1.0000", "discount_rate": "0" });
        let mut data = data.as_object().unwrap().clone();
        assert_eq!(check_line_data(SALES_ORDER_LINE, &mut data).len(), 1);
        assert_eq!(check_line_data(SHIPMENT_LINE, &mut data).len(), 1);
        assert!(check_line_data("customer_contact", &mut data).is_empty());
    }

    #[test]
    fn shipment_lot_no_is_trimmed() {
        let mut data = json!({ "qty": "1", "unit_price": "1", "lot_no": "  L-01 " })
            .as_object()
            .unwrap()
            .clone();
        assert!(check_line_data(SHIPMENT_LINE, &mut data).is_empty());
        assert_eq!(data["lot_no"], "L-01");
        data.insert("lot_no".into(), json!("   "));
        assert!(check_line_data(SHIPMENT_LINE, &mut data).is_empty());
        assert!(!data.contains_key("lot_no"));
        data.insert("lot_no".into(), json!("L\u{7}"));
        assert_eq!(
            check_line_data(SHIPMENT_LINE, &mut data)[0].field_key,
            "lot_no"
        );
    }

    #[test]
    fn totals_are_written_as_padded_strings() {
        let mut data = Map::new();
        let t = amounts::totals(
            [Decimal::from_str("656.0000").unwrap()],
            Decimal::from_str("0.05").unwrap(),
            true,
            0,
        );
        write_totals(&mut data, &t);
        assert_eq!(
            Value::Object(data),
            json!({ "subtotal": "625.0000", "tax_amount": "31.0000", "total": "656.0000" })
        );
    }

    #[test]
    fn orders_and_shipments_are_priced() {
        assert!(recalculates_lines(SALES_ORDER));
        assert!(recalculates_lines(SHIPMENT));
        assert!(priced_line(SALES_ORDER_LINE));
        assert!(priced_line(SHIPMENT_LINE));
        assert!(!recalculates_lines("purchase_order"));
        assert!(recalculates_lines(SALES_RETURN));
        assert!(priced_line(SALES_RETURN_LINE));
        assert!(!with_amounts(SALES_RETURN).unwrap().prefills_tax);
    }
}

//! 進貨模組（採購單、進貨單、進貨退回，docs/architecture.md §8.8）存檔時的規則。
//!
//! 目前只有進貨退回從進貨單帶入系統維護的欄位（[`derive_return`]）：表頭的 `supplier_id`、
//! `currency_code`、`exchange_rate`、`tax_type`，明細的 `product_id`、`lot_no`、`unit_price`。
//! 這些欄位是唯讀的系統欄位（request 帶了一律忽略，§7.1），資料表上為 `NOT NULL`，所以存檔時一定要帶入。
//! 金額計算、退回數量上限、過帳與作廢在之後的 issue 加入。

use std::collections::HashMap;

use hammer_db::purchasing::{self as db, ReceiptForReturn, ReceiptLineForReturn};
use hammer_db::sqlx::PgConnection;
use hammer_shared::Id;
use hammer_shared::schema::{DataType, FieldValue, parse_value};
use serde_json::{Map, Value};

use crate::CoreError;
use crate::record_data::FieldError;
use crate::record_lines::LineError;

/// 採購單的資料類別代碼。
pub const PURCHASE_ORDER: &str = "purchase_order";
/// 進貨單的資料類別代碼。
pub const GOODS_RECEIPT: &str = "goods_receipt";
/// 進貨退回的資料類別代碼。
pub const PURCHASE_RETURN: &str = "purchase_return";
/// 進貨退回明細的明細子表代碼。
pub const PURCHASE_RETURN_LINE: &str = "purchase_return_line";

const GOODS_RECEIPT_ID: &str = "goods_receipt_id";
const GOODS_RECEIPT_LINE_ID: &str = "goods_receipt_line_id";

/// 進貨單不可更換（§8.8 `purchase_returns`）。
pub const MSG_RECEIPT_FIXED: &str = "建立後不可更換進貨單";
/// 進貨單是草稿。
pub const MSG_RECEIPT_DRAFT: &str = "進貨單尚未過帳";
/// 進貨單已作廢。
pub const MSG_RECEIPT_VOID: &str = "進貨單已作廢";
/// 明細不屬於表頭的進貨單。
pub const MSG_NOT_RECEIPT_LINE: &str = "不是此進貨單的明細";

/// 進貨退回存檔時從進貨單帶入（新增與修改都執行；`entity_key` 不是 [`PURCHASE_RETURN`] 時不做事）。
///
/// - `master`：主表正規化後的結果（修改時已與既有資料合併）；`existing`：修改前的資料。
/// - `lines`：通過欄位驗證的進貨退回明細 `(request 中的位置, data)`。
///
/// 進貨單不存在或已刪除時不帶入，交給之後的關聯檢查回報；進貨單不是已過帳、修改時更換了進貨單 →
/// 主表的 `goods_receipt_id` 錯誤；明細不屬於該進貨單 → 該列的 `goods_receipt_line_id` 錯誤。
pub(crate) async fn derive_return(
    conn: &mut PgConnection,
    entity_key: &str,
    master: Result<Map<String, Value>, Vec<FieldError>>,
    existing: Option<&Map<String, Value>>,
    lines: Vec<(usize, &mut Map<String, Value>)>,
) -> Result<(Result<Map<String, Value>, Vec<FieldError>>, Vec<LineError>), CoreError> {
    if entity_key != PURCHASE_RETURN {
        return Ok((master, Vec::new()));
    }
    let receipt_id = match (&master, existing) {
        (Ok(data), _) => relation_id(data.get(GOODS_RECEIPT_ID)),
        (Err(_), Some(existing)) => relation_id(existing.get(GOODS_RECEIPT_ID)),
        (Err(_), None) => None,
    };
    let Some(receipt_id) = receipt_id else {
        return Ok((master, Vec::new()));
    };
    let Some(receipt) = db::find_receipt_for_return(&mut *conn, receipt_id).await? else {
        return Ok((master, Vec::new()));
    };

    let master = match master {
        Ok(mut data) => {
            let changed =
                existing.is_some_and(|e| relation_id(e.get(GOODS_RECEIPT_ID)) != Some(receipt_id));
            match receipt_errors(&receipt, changed) {
                Some(error) => Err(vec![error]),
                None => {
                    apply_receipt(&mut data, &receipt);
                    Ok(data)
                }
            }
        }
        Err(mut errors) => {
            errors.extend(receipt_errors(&receipt, false));
            Err(errors)
        }
    };

    let ids: Vec<Id> = lines
        .iter()
        .filter_map(|(_, data)| relation_id(data.get(GOODS_RECEIPT_LINE_ID)))
        .collect();
    let found: HashMap<Id, ReceiptLineForReturn> = if ids.is_empty() {
        HashMap::new()
    } else {
        db::list_receipt_lines_for_return(&mut *conn, receipt_id, &ids)
            .await?
            .into_iter()
            .map(|row| (row.id, row))
            .collect()
    };
    let mut line_errors = Vec::new();
    for (index, data) in lines {
        match relation_id(data.get(GOODS_RECEIPT_LINE_ID)).and_then(|id| found.get(&id)) {
            Some(row) => apply_receipt_line(data, row),
            None => line_errors.push(LineError::field(
                PURCHASE_RETURN_LINE,
                index,
                GOODS_RECEIPT_LINE_ID,
                MSG_NOT_RECEIPT_LINE,
            )),
        }
    }
    Ok((master, line_errors))
}

/// 進貨單不能用於退回的原因：修改時更換了進貨單、進貨單不是已過帳。
fn receipt_errors(receipt: &ReceiptForReturn, changed: bool) -> Option<FieldError> {
    let message = if changed {
        MSG_RECEIPT_FIXED
    } else {
        match receipt.status.as_str() {
            "posted" if receipt.exchange_rate.is_some() => return None,
            "void" => MSG_RECEIPT_VOID,
            _ => MSG_RECEIPT_DRAFT,
        }
    };
    Some(FieldError {
        field_key: GOODS_RECEIPT_ID.into(),
        message: message.into(),
    })
}

/// 表頭帶入進貨單的廠商、幣別、匯率、稅別。
fn apply_receipt(data: &mut Map<String, Value>, receipt: &ReceiptForReturn) {
    data.insert(
        "supplier_id".into(),
        Value::String(receipt.supplier_id.to_string()),
    );
    data.insert(
        "currency_code".into(),
        Value::String(receipt.currency_code.clone()),
    );
    if let Some(rate) = receipt.exchange_rate {
        data.insert("exchange_rate".into(), Value::String(rate.to_string()));
    }
    data.insert("tax_type".into(), Value::String(receipt.tax_type.clone()));
}

/// 明細帶入進貨明細的商品、批號、單價。
fn apply_receipt_line(data: &mut Map<String, Value>, row: &ReceiptLineForReturn) {
    data.insert(
        "product_id".into(),
        Value::String(row.product_id.to_string()),
    );
    match &row.lot_no {
        Some(lot_no) => data.insert("lot_no".into(), Value::String(lot_no.clone())),
        None => data.remove("lot_no"),
    };
    data.insert(
        "unit_price".into(),
        Value::String(row.unit_price.to_string()),
    );
}

fn relation_id(value: Option<&Value>) -> Option<Id> {
    match parse_value(DataType::Relation, value?) {
        Ok(Some(FieldValue::Relation(id))) => Some(id),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;

    fn receipt(status: &str, rate: Option<&str>) -> ReceiptForReturn {
        ReceiptForReturn {
            id: Uuid::nil(),
            status: status.into(),
            supplier_id: Uuid::from_u128(7),
            currency_code: "USD".into(),
            exchange_rate: rate.map(|r| r.parse().unwrap()),
            tax_type: "zero_rated".into(),
        }
    }

    #[test]
    fn only_posted_receipts_can_be_returned() {
        assert_eq!(
            receipt_errors(&receipt("posted", Some("32.150000")), false),
            None
        );
        let message = |r: &ReceiptForReturn, changed| receipt_errors(r, changed).unwrap().message;
        assert_eq!(message(&receipt("draft", None), false), MSG_RECEIPT_DRAFT);
        assert_eq!(
            message(&receipt("void", Some("1")), false),
            MSG_RECEIPT_VOID
        );
        assert_eq!(
            message(&receipt("posted", Some("1")), true),
            MSG_RECEIPT_FIXED
        );
        assert_eq!(
            receipt_errors(&receipt("posted", None), false)
                .unwrap()
                .field_key,
            GOODS_RECEIPT_ID
        );
    }

    #[test]
    fn header_copies_receipt_values() {
        let mut data = json!({ "goods_receipt_id": "x", "supplier_id": "ignored" })
            .as_object()
            .unwrap()
            .clone();
        apply_receipt(&mut data, &receipt("posted", Some("32.150000")));
        assert_eq!(
            Value::Object(data),
            json!({
                "goods_receipt_id": "x",
                "supplier_id": Uuid::from_u128(7).to_string(),
                "currency_code": "USD",
                "exchange_rate": "32.150000",
                "tax_type": "zero_rated",
            })
        );
    }

    #[test]
    fn line_copies_receipt_line_values() {
        let row = ReceiptLineForReturn {
            id: Uuid::nil(),
            product_id: Uuid::from_u128(9),
            lot_no: Some("L-01".into()),
            unit_price: "12.3450".parse().unwrap(),
            lot_id: None,
        };
        let mut data = json!({ "qty": "2.0000" }).as_object().unwrap().clone();
        apply_receipt_line(&mut data, &row);
        assert_eq!(
            Value::Object(data.clone()),
            json!({
                "qty": "2.0000",
                "product_id": Uuid::from_u128(9).to_string(),
                "lot_no": "L-01",
                "unit_price": "12.3450",
            })
        );
        // 服務類沒有批號
        let service = ReceiptLineForReturn {
            lot_no: None,
            ..row
        };
        apply_receipt_line(&mut data, &service);
        assert!(!data.contains_key("lot_no"));
    }

    #[test]
    fn relation_id_parses_uuid_strings() {
        let id = Uuid::from_u128(3);
        assert_eq!(relation_id(Some(&json!(id.to_string()))), Some(id));
        assert_eq!(relation_id(Some(&json!("not-a-uuid"))), None);
        assert_eq!(relation_id(None), None);
    }
}

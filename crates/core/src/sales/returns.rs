//! 銷貨退回存檔時從出貨單帶入系統維護的欄位（[`derive_return`]）：表頭的 `customer_id`、`tax_type`、
//! `tax_included`，明細的 `product_id`、`unit_price`、`discount_rate`。這些欄位是唯讀的系統欄位（request
//! 帶了一律忽略，§7.1），資料表上為 `NOT NULL`，所以存檔時一定要帶入。
//! 退回數量上限、退回日期、過帳與作廢在之後的 issue 加入（#150）。

use std::collections::HashMap;

use hammer_db::sales::{self as db, ShipmentForReturn, ShipmentLineForReturn};
use hammer_db::sqlx::PgConnection;
use hammer_shared::Id;
use hammer_shared::schema::{DataType, FieldValue, parse_value};
use serde_json::{Map, Value};

use super::{SALES_RETURN, SALES_RETURN_LINE};
use crate::CoreError;
use crate::record_data::FieldError;
use crate::record_lines::LineError;

const SHIPMENT_ID: &str = "shipment_id";
const SHIPMENT_LINE_ID: &str = "shipment_line_id";

/// 出貨單不可更換（§8.9 `sales_returns`）。
pub const MSG_SHIPMENT_FIXED: &str = "建立後不可更換出貨單";
/// 出貨單是草稿。
pub const MSG_SHIPMENT_DRAFT: &str = "出貨單尚未過帳";
/// 出貨單已作廢。
pub const MSG_SHIPMENT_VOID: &str = "出貨單已作廢";
/// 明細不屬於表頭的出貨單。
pub const MSG_NOT_SHIPMENT_LINE: &str = "不是此出貨單的明細";

/// 銷貨退回存檔時從出貨單帶入（新增與修改都執行；`entity_key` 不是 [`SALES_RETURN`] 時不做事）。
///
/// - `master`：主表正規化後的結果（修改時已與既有資料合併）；`existing`：修改前的資料。
/// - `lines`：通過欄位驗證的銷貨退回明細 `(request 中的位置, data)`。
///
/// 出貨單不存在或已刪除時不帶入，交給之後的關聯檢查回報；出貨單不是已過帳、修改時更換了出貨單 →
/// 主表的 `shipment_id` 錯誤；明細不屬於該出貨單 → 該列的 `shipment_line_id` 錯誤。
pub async fn derive_return(
    conn: &mut PgConnection,
    entity_key: &str,
    master: Result<Map<String, Value>, Vec<FieldError>>,
    existing: Option<&Map<String, Value>>,
    lines: Vec<(usize, &mut Map<String, Value>)>,
) -> Result<(Result<Map<String, Value>, Vec<FieldError>>, Vec<LineError>), CoreError> {
    if entity_key != SALES_RETURN {
        return Ok((master, Vec::new()));
    }
    let shipment_id = match (&master, existing) {
        (Ok(data), _) => relation_id(data.get(SHIPMENT_ID)),
        (Err(_), Some(existing)) => relation_id(existing.get(SHIPMENT_ID)),
        (Err(_), None) => None,
    };
    let Some(shipment_id) = shipment_id else {
        return Ok((master, Vec::new()));
    };
    let Some(shipment) = db::find_shipment_for_return(&mut *conn, shipment_id).await? else {
        return Ok((master, Vec::new()));
    };

    let master = match master {
        Ok(mut data) => {
            let changed =
                existing.is_some_and(|e| relation_id(e.get(SHIPMENT_ID)) != Some(shipment_id));
            match shipment_error(&shipment, changed) {
                Some(error) => Err(vec![error]),
                None => {
                    apply_shipment(&mut data, &shipment);
                    Ok(data)
                }
            }
        }
        Err(mut errors) => {
            errors.extend(shipment_error(&shipment, false));
            Err(errors)
        }
    };

    let ids: Vec<Id> = lines
        .iter()
        .filter_map(|(_, data)| relation_id(data.get(SHIPMENT_LINE_ID)))
        .collect();
    let found: HashMap<Id, ShipmentLineForReturn> = if ids.is_empty() {
        HashMap::new()
    } else {
        db::list_shipment_lines_for_return(&mut *conn, shipment_id, &ids)
            .await?
            .into_iter()
            .map(|row| (row.id, row))
            .collect()
    };
    let mut line_errors = Vec::new();
    for (index, data) in lines {
        match relation_id(data.get(SHIPMENT_LINE_ID)).and_then(|id| found.get(&id)) {
            Some(row) => apply_shipment_line(data, row),
            None => line_errors.push(LineError::field(
                SALES_RETURN_LINE,
                index,
                SHIPMENT_LINE_ID,
                MSG_NOT_SHIPMENT_LINE,
            )),
        }
    }
    Ok((master, line_errors))
}

/// 出貨單不能用於退回的原因：修改時更換了出貨單、出貨單不是已過帳。
fn shipment_error(shipment: &ShipmentForReturn, changed: bool) -> Option<FieldError> {
    let message = if changed {
        MSG_SHIPMENT_FIXED
    } else {
        match shipment.status.as_str() {
            "posted" => return None,
            "void" => MSG_SHIPMENT_VOID,
            _ => MSG_SHIPMENT_DRAFT,
        }
    };
    Some(FieldError {
        field_key: SHIPMENT_ID.into(),
        message: message.into(),
    })
}

/// 表頭帶入出貨單的客戶、稅別、單價含稅。
fn apply_shipment(data: &mut Map<String, Value>, shipment: &ShipmentForReturn) {
    data.insert(
        "customer_id".into(),
        Value::String(shipment.customer_id.to_string()),
    );
    data.insert("tax_type".into(), Value::String(shipment.tax_type.clone()));
    data.insert("tax_included".into(), Value::Bool(shipment.tax_included));
}

/// 明細帶入出貨明細的商品、單價、折扣率。
fn apply_shipment_line(data: &mut Map<String, Value>, row: &ShipmentLineForReturn) {
    data.insert(
        "product_id".into(),
        Value::String(row.product_id.to_string()),
    );
    data.insert(
        "unit_price".into(),
        Value::String(row.unit_price.to_string()),
    );
    data.insert(
        "discount_rate".into(),
        Value::String(row.discount_rate.to_string()),
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

    fn shipment(status: &str) -> ShipmentForReturn {
        ShipmentForReturn {
            id: Uuid::nil(),
            status: status.into(),
            customer_id: Uuid::from_u128(7),
            tax_type: "taxable".into(),
            tax_included: true,
        }
    }

    #[test]
    fn only_posted_shipments_can_be_returned() {
        assert_eq!(shipment_error(&shipment("posted"), false), None);
        let message = |s: &ShipmentForReturn, changed| shipment_error(s, changed).unwrap().message;
        assert_eq!(message(&shipment("draft"), false), MSG_SHIPMENT_DRAFT);
        assert_eq!(message(&shipment("void"), false), MSG_SHIPMENT_VOID);
        assert_eq!(message(&shipment("posted"), true), MSG_SHIPMENT_FIXED);
        assert_eq!(
            shipment_error(&shipment("draft"), false).unwrap().field_key,
            SHIPMENT_ID
        );
    }

    #[test]
    fn header_copies_shipment_values() {
        let mut data =
            json!({ "shipment_id": "x", "customer_id": "ignored", "tax_included": false })
                .as_object()
                .unwrap()
                .clone();
        apply_shipment(&mut data, &shipment("posted"));
        assert_eq!(
            Value::Object(data),
            json!({
                "shipment_id": "x",
                "customer_id": Uuid::from_u128(7).to_string(),
                "tax_type": "taxable",
                "tax_included": true,
            })
        );
    }

    #[test]
    fn line_copies_shipment_line_values() {
        let row = ShipmentLineForReturn {
            id: Uuid::nil(),
            product_id: Uuid::from_u128(9),
            unit_price: "105.0000".parse().unwrap(),
            discount_rate: "0.1000".parse().unwrap(),
        };
        let mut data = json!({ "qty": "2.0000", "unit_price": "1" })
            .as_object()
            .unwrap()
            .clone();
        apply_shipment_line(&mut data, &row);
        assert_eq!(
            Value::Object(data),
            json!({
                "qty": "2.0000",
                "product_id": Uuid::from_u128(9).to_string(),
                "unit_price": "105.0000",
                "discount_rate": "0.1000",
            })
        );
    }

    #[test]
    fn relation_id_parses_uuid_strings() {
        let id = Uuid::from_u128(3);
        assert_eq!(relation_id(Some(&json!(id.to_string()))), Some(id));
        assert_eq!(relation_id(Some(&json!("not-a-uuid"))), None);
        assert_eq!(relation_id(None), None);
    }
}

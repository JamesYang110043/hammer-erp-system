//! 基本資料（單位、商品分類、商品、客戶、廠商）在通用欄位驗證之外的業務規則
//! （docs/architecture.md §7.2「各系統欄位的系統限制」、§8.6、§8.7）。
//!
//! 由 [`crate::records`] 在系統資料類別（`storage = table`）寫入與刪除時呼叫：
//!
//! - [`check_data`]（純邏輯）：通過 [`crate::record_data::normalize_record_data`] 之後，檢查系統欄位的
//!   固定限制。這些限制也由 migration 寫入欄位定義（`is_required`、`validation`、`options`，不可修改），
//!   這裡再檢查一次，不依賴欄位定義的內容；另外檢查欄位定義無法表達的規則（統一編號格式）。
//!   `code` 去除前後空白後儲存
//! - [`check_references`]：需要查詢資料庫的規則——廠商的預設幣別必須有效、商品分類的上層分類不可形成循環
//! - [`check_delete`]：還有未刪除商品使用的單位、分類，有子分類的分類，以及有庫存異動的商品不能刪除
//! - 商品被庫存引用時的限制（§7.2）：有庫存異動（`inventory_transactions` 有該商品的資料，不論來源單據
//!   之後是否作廢）的商品不可刪除、不可從 `goods` 改為 `service`、不可修改 `unit_id`。修改與刪除都在
//!   以 `FOR UPDATE` 鎖定商品之後檢查（過帳以 `FOR SHARE` 鎖定商品，兩者互斥）
//! - [`check_lines`]（純邏輯）：系統明細子表（聯絡人）一起存檔的所有明細之間的規則——同一個客戶或廠商
//!   最多一位主要聯絡人（`is_primary = true`）。聯絡人每一列的系統欄位限制也由 [`check_data`] 檢查
//!
//! 錯誤一律 `422 VALIDATION`：欄位的錯誤為 [`CoreError::InvalidFields`]（`details.fields`），明細的錯誤
//! 為 [`LineError`]（`details.lines`，由 [`crate::records`] 合併），
//! 刪除被擋下為 [`CoreError::RecordInUse`]（`details.fields` 為空，另有使用中的數量）。
//!
//! 修改資料時只檢查值有變動的欄位，避免規則加上之前存入的資料在修改其他欄位時被擋下（與
//! `default_currency_code`「只在值有變動時檢查」相同，§7.2）。
//!
//! 不做：統一編號檢查碼驗證；客戶、廠商被單據引用時的刪除限制（M4）。

use std::str::FromStr;

use hammer_db::lock::advisory_xact_lock;
use hammer_db::master_data as db;
use hammer_db::sqlx::PgTransaction;
use hammer_shared::Id;
use hammer_shared::decimal::Decimal;
use serde_json::{Map, Value};

use crate::CoreError;
use crate::record_data::FieldError;
use crate::record_lines::LineError;

/// 單位的資料類別代碼。
pub const UNIT: &str = "unit";
/// 商品分類的資料類別代碼。
pub const PRODUCT_CATEGORY: &str = "product_category";
/// 商品的資料類別代碼。
pub const PRODUCT: &str = "product";
/// 客戶的資料類別代碼。
pub const CUSTOMER: &str = "customer";
/// 廠商的資料類別代碼。
pub const SUPPLIER: &str = "supplier";
/// 客戶聯絡人的明細子表代碼。
pub const CUSTOMER_CONTACT: &str = "customer_contact";
/// 廠商聯絡人的明細子表代碼。
pub const SUPPLIER_CONTACT: &str = "supplier_contact";
/// 聯絡人的「主要聯絡人」欄位。
pub const IS_PRIMARY: &str = "is_primary";

/// 商品 `product_type` 的選項（§8.6）。
pub const PRODUCT_TYPES: &[&str] = &["goods", "service"];
/// 商品 `tax_type` 的選項，與 `tax_rates.code` 相同（§8.6）。
pub const TAX_TYPES: &[&str] = &["taxable", "zero_rated", "exempt"];

/// 修改商品分類的上層分類時使用的 advisory lock 鍵值：兩個 transaction 同時修改不同分類的上層分類時，
/// 各自檢查都不會發現循環，合起來卻形成循環，所以修改上層分類一律依序進行。
pub const CATEGORY_TREE_LOCK_KEY: i64 = 0x4841_4d4d_4341_5454; // "HAMMCATT"

pub const MSG_REQUIRED: &str = "此欄位為必填";
pub const MSG_NEGATIVE: &str = "不可小於 0";
pub const MSG_TAX_ID: &str = "統一編號必須是 8 位數字";
pub const MSG_CURRENCY: &str = "不是有效的幣別";
pub const MSG_CATEGORY_CYCLE: &str = "上層分類不可是自己或自己的下層分類";
pub const MSG_CUSTOMER_PRIMARY: &str = "每個客戶最多只能有一位主要聯絡人";
pub const MSG_SUPPLIER_PRIMARY: &str = "每個廠商最多只能有一位主要聯絡人";
pub const MSG_PRODUCT_DELETE_STOCK: &str = "此商品已有庫存異動，無法刪除，請改為停用";
pub const MSG_PRODUCT_TYPE_STOCK: &str = "此商品已有庫存異動，無法改為服務";
pub const MSG_PRODUCT_UNIT_STOCK: &str = "此商品已有庫存異動，無法修改單位";

/// 有 `code` 欄位的資料類別（§7.2）。
const WITH_CODE: &[&str] = &[UNIT, PRODUCT, CUSTOMER, SUPPLIER];

fn invalid_option(value: &str) -> String {
    format!("「{value}」不是有效的選項")
}

/// 檢查 `data`（正規化後要寫入的完整資料）中系統欄位的固定限制，回傳所有錯誤（依欄位檢查的順序）。
/// `existing` 為修改前的資料（新增時為 `None`），修改時只檢查值有變動的欄位。
///
/// `code` 去除前後空白後寫回 `data`。不是基本資料的資料類別不檢查。
pub fn check_data(
    entity_key: &str,
    data: &mut Map<String, Value>,
    existing: Option<&Map<String, Value>>,
) -> Vec<FieldError> {
    if WITH_CODE.contains(&entity_key) {
        trim_code(data);
    }
    let mut check = Check {
        data,
        existing,
        errors: Vec::new(),
    };
    match entity_key {
        UNIT => check.required("code"),
        PRODUCT_CATEGORY => {}
        PRODUCT => {
            check.required("code");
            check.option("product_type", PRODUCT_TYPES);
            check.option("tax_type", TAX_TYPES);
            check.required("unit_id");
            check.non_negative("default_sale_price");
            check.non_negative("safety_stock");
        }
        CUSTOMER | SUPPLIER => {
            check.required("code");
            check.tax_id("tax_id");
            check.non_negative("payment_terms_days");
            check.non_negative("credit_limit");
            if entity_key == SUPPLIER {
                check.required("default_currency_code");
            }
        }
        CUSTOMER_CONTACT | SUPPLIER_CONTACT => {
            check.required("name");
            check.required(IS_PRIMARY);
        }
        _ => {}
    }
    check.errors
}

/// 檢查系統明細子表 `detail_key` 一起存檔的明細（`(在 request 陣列中的位置, 正規化後的資料)`，依順序）
/// 之間的規則，回傳所有錯誤：
///
/// - 聯絡人：`is_primary = true` 最多一列；第二列起的每一列回傳錯誤（`field_key = is_primary`，§7.2）
///
/// 其他明細子表不檢查。
pub fn check_lines(detail_key: &str, lines: &[(usize, &Map<String, Value>)]) -> Vec<LineError> {
    let message = match detail_key {
        CUSTOMER_CONTACT => MSG_CUSTOMER_PRIMARY,
        SUPPLIER_CONTACT => MSG_SUPPLIER_PRIMARY,
        _ => return Vec::new(),
    };
    lines
        .iter()
        .filter(|(_, data)| data.get(IS_PRIMARY) == Some(&Value::Bool(true)))
        .skip(1)
        .map(|(index, _)| LineError::field(detail_key, *index, IS_PRIMARY, message))
        .collect()
}

/// `code` 去除前後空白；去除後為空時移除（之後由必填檢查擋下）。
fn trim_code(data: &mut Map<String, Value>) {
    let Some(Value::String(code)) = data.get("code") else {
        return;
    };
    let trimmed = code.trim();
    if trimmed.is_empty() {
        data.remove("code");
    } else if trimmed.len() != code.len() {
        let trimmed = trimmed.to_owned();
        data.insert("code".to_owned(), Value::String(trimmed));
    }
}

struct Check<'a> {
    data: &'a Map<String, Value>,
    existing: Option<&'a Map<String, Value>>,
    errors: Vec<FieldError>,
}

impl Check<'_> {
    /// 要檢查的值：新增時一律檢查；修改時只檢查有變動的欄位。不檢查時為 `None`。
    fn value(&self, key: &str) -> Option<Option<&Value>> {
        let value = self.data.get(key);
        match self.existing {
            Some(existing) if existing.get(key) == value => None,
            _ => Some(value),
        }
    }

    fn error(&mut self, key: &str, message: impl Into<String>) {
        self.errors.push(FieldError {
            field_key: key.to_owned(),
            message: message.into(),
        });
    }

    fn required(&mut self, key: &str) {
        if let Some(None) = self.value(key) {
            self.error(key, MSG_REQUIRED);
        }
    }

    /// 必填，且值必須是 `options` 之一。
    fn option(&mut self, key: &str, options: &[&str]) {
        match self.value(key) {
            None => {}
            Some(None) => self.error(key, MSG_REQUIRED),
            Some(Some(v)) => match v.as_str() {
                Some(s) if options.contains(&s) => {}
                Some(s) => self.error(key, invalid_option(s)),
                None => self.error(key, invalid_option(&v.to_string())),
            },
        }
    }

    /// 有值時不可小於 0。整數為 JSON number，小數與金額為字串（§7.3）。
    fn non_negative(&mut self, key: &str) {
        let Some(Some(v)) = self.value(key) else {
            return;
        };
        let n = match v {
            Value::Number(n) => n.as_i64().map(Decimal::from),
            Value::String(s) => Decimal::from_str(s).ok(),
            _ => None,
        };
        if n.is_none_or(|n| n.is_sign_negative() && !n.is_zero()) {
            self.error(key, MSG_NEGATIVE);
        }
    }

    /// 有值時必須是 8 位數字（不驗證檢查碼）。
    fn tax_id(&mut self, key: &str) {
        let Some(Some(v)) = self.value(key) else {
            return;
        };
        if !v.as_str().is_some_and(is_tax_id) {
            self.error(key, MSG_TAX_ID);
        }
    }
}

/// 是否為 8 位數字（ASCII `0`–`9`）。
pub fn is_tax_id(s: &str) -> bool {
    s.len() == 8 && s.bytes().all(|b| b.is_ascii_digit())
}

/// 把分類 `record_id` 的上層分類設為 `new_parent` 是否會形成循環。`ancestors` 為 `new_parent` 本身與
/// 它往上的所有上層分類（[`hammer_db::master_data::category_ancestors`]）。
///
/// 指向自己（`new_parent == record_id`）也是循環。
pub fn creates_cycle(record_id: Id, ancestors: &[Id]) -> bool {
    ancestors.contains(&record_id)
}

/// 需要查詢資料庫的寫入規則。`record_id` 為修改的資料（新增時為 `None`），`data` 為要寫入的完整資料，
/// `existing` 為修改前的資料；只檢查值有變動的欄位。
///
/// - 廠商：`default_currency_code` 必須是存在且啟用中的幣別
/// - 商品分類：修改時 `parent_id` 不可指向自己或自己的下層分類（新增的分類還沒有下層分類，不會形成
///   循環）。檢查前取得 [`CATEGORY_TREE_LOCK_KEY`]
///
/// 違反 → [`CoreError::InvalidFields`]。
pub(crate) async fn check_references(
    tx: &mut PgTransaction<'_>,
    entity_key: &str,
    record_id: Option<Id>,
    data: &Map<String, Value>,
    existing: Option<&Map<String, Value>>,
) -> Result<(), CoreError> {
    let changed = |key: &str| {
        let value = data.get(key);
        let changed = existing.is_none_or(|e| e.get(key) != value);
        value.filter(|_| changed)
    };
    let mut errors = Vec::new();
    match entity_key {
        SUPPLIER => {
            if let Some(code) = changed("default_currency_code") {
                let active = match code.as_str() {
                    Some(code) => db::currency_is_active(&mut **tx, code).await?,
                    None => false,
                };
                if !active {
                    errors.push(FieldError {
                        field_key: "default_currency_code".into(),
                        message: MSG_CURRENCY.into(),
                    });
                }
            }
        }
        PRODUCT => {
            // 修改時才有庫存異動（新增的商品還沒有異動）
            if let (Some(record_id), Some(existing)) = (record_id, existing) {
                let to_service = changed("product_type").is_some_and(|v| v == "service")
                    && existing.get("product_type").is_some_and(|v| v == "goods");
                let unit_changed = changed("unit_id").is_some();
                if (to_service || unit_changed)
                    && db::count_product_transactions(&mut **tx, record_id).await? > 0
                {
                    if to_service {
                        errors.push(FieldError {
                            field_key: "product_type".into(),
                            message: MSG_PRODUCT_TYPE_STOCK.into(),
                        });
                    }
                    if unit_changed {
                        errors.push(FieldError {
                            field_key: "unit_id".into(),
                            message: MSG_PRODUCT_UNIT_STOCK.into(),
                        });
                    }
                }
            }
        }
        PRODUCT_CATEGORY => {
            if let (Some(record_id), Some(parent)) = (record_id, changed("parent_id")) {
                let parent = parent.as_str().and_then(|s| Id::parse_str(s).ok());
                // 已經過 normalize_record_data 與 check_relations，格式一定正確
                if let Some(parent) = parent {
                    advisory_xact_lock(&mut **tx, CATEGORY_TREE_LOCK_KEY).await?;
                    let ancestors = db::category_ancestors(&mut **tx, parent).await?;
                    if creates_cycle(record_id, &ancestors) {
                        errors.push(FieldError {
                            field_key: "parent_id".into(),
                            message: MSG_CATEGORY_CYCLE.into(),
                        });
                    }
                }
            }
        }
        _ => {}
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(CoreError::InvalidFields(errors))
    }
}

/// 刪除前的使用狀況。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    /// 使用中的未刪除商品數。
    pub products: i64,
    /// 未刪除的子分類數。
    pub children: i64,
    /// 商品的庫存異動筆數。
    pub transactions: i64,
}

/// 依使用狀況決定能否刪除：都沒有使用時為 `None`，否則為 [`CoreError::RecordInUse`]。
pub fn delete_blocked(entity_key: &str, usage: Usage) -> Option<CoreError> {
    if usage.transactions > 0 {
        return Some(CoreError::RecordInUse {
            message: MSG_PRODUCT_DELETE_STOCK.into(),
            products: 0,
            children: 0,
            transactions: usage.transactions,
        });
    }
    if usage.products <= 0 && usage.children <= 0 {
        return None;
    }
    let subject = if entity_key == UNIT {
        "此單位"
    } else {
        "此分類"
    };
    let mut reasons = Vec::new();
    if usage.products > 0 {
        reasons.push(format!("{} 筆商品使用中", usage.products));
    }
    if usage.children > 0 {
        reasons.push(format!("{} 個子分類", usage.children));
    }
    Some(CoreError::RecordInUse {
        message: format!("{subject}還有 {}，無法刪除", reasons.join("、")),
        products: usage.products.max(0),
        children: usage.children.max(0),
        transactions: 0,
    })
}

/// 刪除單位、商品分類前，檢查是否還有未刪除的商品使用它，或分類還有子分類；刪除商品前檢查是否已有
/// 庫存異動 → [`CoreError::RecordInUse`]。其他資料類別不檢查。
///
/// 必須在已鎖定（`FOR UPDATE`）要刪除的資料之後呼叫：新增、修改商品或分類時，關聯檢查會以 `FOR SHARE`
/// 鎖定所指的單位、分類（[`crate::relations::check_relations`]），所以計數之後不會再有新的使用者。
pub(crate) async fn check_delete(
    tx: &mut PgTransaction<'_>,
    entity_key: &str,
    id: Id,
) -> Result<(), CoreError> {
    let usage = match entity_key {
        UNIT => Usage {
            products: db::count_products_by_unit(&mut **tx, id).await?,
            ..Usage::default()
        },
        PRODUCT_CATEGORY => Usage {
            products: db::count_products_by_category(&mut **tx, id).await?,
            children: db::count_child_categories(&mut **tx, id).await?,
            ..Usage::default()
        },
        PRODUCT => Usage {
            transactions: db::count_product_transactions(&mut **tx, id).await?,
            ..Usage::default()
        },
        _ => return Ok(()),
    };
    match delete_blocked(entity_key, usage) {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hammer_shared::ApiError;
    use hammer_shared::new_id;
    use serde_json::json;

    fn map(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    fn keys(errors: &[FieldError]) -> Vec<&str> {
        errors.iter().map(|e| e.field_key.as_str()).collect()
    }

    fn valid_product() -> Map<String, Value> {
        map(json!({
            "code": "A001", "name": "螺絲", "product_type": "goods", "tax_type": "taxable",
            "unit_id": "01a1e516-3401-7432-acb3-9cc3ac6a7db9",
            "default_sale_price": "10.0000", "safety_stock": "0.0000",
        }))
    }

    #[test]
    fn code_is_required_for_entities_with_code() {
        for entity in [UNIT, PRODUCT, CUSTOMER, SUPPLIER] {
            let mut data = map(json!({ "name": "x" }));
            let errors = check_data(entity, &mut data, None);
            assert!(keys(&errors).contains(&"code"), "{entity}: {errors:?}");
        }
        let mut data = map(json!({ "name": "x" }));
        assert!(check_data(PRODUCT_CATEGORY, &mut data, None).is_empty());
    }

    #[test]
    fn code_is_trimmed_and_blank_code_is_missing() {
        let mut data = map(json!({ "code": "  pcs ", "name": "個" }));
        assert!(check_data(UNIT, &mut data, None).is_empty());
        assert_eq!(data["code"], "pcs");

        let mut data = map(json!({ "code": "   ", "name": "個" }));
        assert_eq!(keys(&check_data(UNIT, &mut data, None)), ["code"]);
        assert!(data.get("code").is_none());
    }

    #[test]
    fn valid_product_passes() {
        let mut data = valid_product();
        assert_eq!(check_data(PRODUCT, &mut data, None), vec![]);
    }

    #[test]
    fn product_type_must_be_goods_or_service() {
        for ok in ["goods", "service"] {
            let mut data = valid_product();
            data.insert("product_type".into(), json!(ok));
            assert!(check_data(PRODUCT, &mut data, None).is_empty());
        }
        let mut data = valid_product();
        data.insert("product_type".into(), json!("bundle"));
        let errors = check_data(PRODUCT, &mut data, None);
        assert_eq!(keys(&errors), ["product_type"]);
        assert_eq!(errors[0].message, "「bundle」不是有效的選項");

        let mut data = valid_product();
        data.remove("product_type");
        let errors = check_data(PRODUCT, &mut data, None);
        assert_eq!(keys(&errors), ["product_type"]);
        assert_eq!(errors[0].message, MSG_REQUIRED);
    }

    #[test]
    fn tax_type_must_be_a_tax_rate_code() {
        for ok in TAX_TYPES {
            let mut data = valid_product();
            data.insert("tax_type".into(), json!(ok));
            assert!(check_data(PRODUCT, &mut data, None).is_empty());
        }
        for bad in [json!("vat"), json!(5)] {
            let mut data = valid_product();
            data.insert("tax_type".into(), bad);
            assert_eq!(keys(&check_data(PRODUCT, &mut data, None)), ["tax_type"]);
        }
    }

    #[test]
    fn product_unit_is_required() {
        let mut data = valid_product();
        data.remove("unit_id");
        let errors = check_data(PRODUCT, &mut data, None);
        assert_eq!(keys(&errors), ["unit_id"]);
        assert_eq!(errors[0].message, MSG_REQUIRED);
    }

    #[test]
    fn product_amounts_are_not_negative() {
        let mut data = valid_product();
        data.insert("default_sale_price".into(), json!("-0.0001"));
        data.insert("safety_stock".into(), json!("-5.0000"));
        let errors = check_data(PRODUCT, &mut data, None);
        assert_eq!(keys(&errors), ["default_sale_price", "safety_stock"]);
        assert!(errors.iter().all(|e| e.message == MSG_NEGATIVE));

        // 0 與「-0」可以
        let mut data = valid_product();
        data.insert("default_sale_price".into(), json!("0.0000"));
        data.insert("safety_stock".into(), json!("-0.0000"));
        assert!(check_data(PRODUCT, &mut data, None).is_empty());
    }

    #[test]
    fn tax_id_must_be_eight_digits() {
        assert!(is_tax_id("04595257"));
        for bad in [
            "1234567",
            "123456789",
            "1234567a",
            "１２３４５６７８",
            "",
            "1234 678",
        ] {
            assert!(!is_tax_id(bad), "{bad}");
        }
        for entity in [CUSTOMER, SUPPLIER] {
            let mut data = map(json!({ "code": "C1", "name": "甲", "tax_id": "04595257" }));
            if entity == SUPPLIER {
                data.insert("default_currency_code".into(), json!("TWD"));
            }
            assert!(check_data(entity, &mut data, None).is_empty(), "{entity}");

            data.insert("tax_id".into(), json!("A1234567"));
            let errors = check_data(entity, &mut data, None);
            assert_eq!(keys(&errors), ["tax_id"], "{entity}");
            assert_eq!(errors[0].message, MSG_TAX_ID);

            // 沒有填（國外客戶）可以
            data.remove("tax_id");
            assert!(check_data(entity, &mut data, None).is_empty(), "{entity}");
        }
    }

    #[test]
    fn payment_terms_and_credit_limit_are_not_negative() {
        for entity in [CUSTOMER, SUPPLIER] {
            let mut data = map(json!({
                "code": "C1", "name": "甲", "payment_terms_days": -1, "credit_limit": "-1.00",
                "default_currency_code": "TWD",
            }));
            let errors = check_data(entity, &mut data, None);
            assert_eq!(
                keys(&errors),
                ["payment_terms_days", "credit_limit"],
                "{entity}"
            );

            let mut data = map(json!({
                "code": "C1", "name": "甲", "payment_terms_days": 0, "credit_limit": "0.00",
                "default_currency_code": "TWD",
            }));
            assert!(check_data(entity, &mut data, None).is_empty(), "{entity}");
        }
    }

    #[test]
    fn supplier_currency_is_required() {
        let mut data = map(json!({ "code": "S1", "name": "乙" }));
        assert_eq!(
            keys(&check_data(SUPPLIER, &mut data, None)),
            ["default_currency_code"]
        );
        let mut data = map(json!({ "code": "C1", "name": "甲" }));
        assert!(check_data(CUSTOMER, &mut data, None).is_empty());
    }

    #[test]
    fn update_checks_only_changed_fields() {
        // 規則加上前存入的統一編號，修改其他欄位時不擋
        let existing = map(json!({ "code": "C1", "name": "甲", "tax_id": "N/A" }));
        let mut data = existing.clone();
        data.insert("name".into(), json!("甲公司"));
        assert!(check_data(CUSTOMER, &mut data, Some(&existing)).is_empty());

        // 修改統一編號時檢查
        data.insert("tax_id".into(), json!("123"));
        assert_eq!(
            keys(&check_data(CUSTOMER, &mut data, Some(&existing))),
            ["tax_id"]
        );

        // 清空必填欄位時檢查
        let existing = valid_product();
        let mut data = existing.clone();
        data.remove("unit_id");
        assert_eq!(
            keys(&check_data(PRODUCT, &mut data, Some(&existing))),
            ["unit_id"]
        );
    }

    #[test]
    fn other_entities_are_not_checked() {
        let mut data = map(json!({ "tax_id": "x", "credit_limit": "-1" }));
        assert!(check_data("repair_order", &mut data, None).is_empty());
    }

    #[test]
    fn contact_name_and_is_primary_are_required() {
        for detail in [CUSTOMER_CONTACT, SUPPLIER_CONTACT] {
            let mut data = map(json!({ "name": "王小明", "is_primary": false }));
            assert!(check_data(detail, &mut data, None).is_empty(), "{detail}");
            let mut data = map(json!({ "title": "經理" }));
            assert_eq!(
                keys(&check_data(detail, &mut data, None)),
                ["name", IS_PRIMARY]
            );
        }
    }

    fn line_indexes(errors: &[LineError]) -> Vec<Option<usize>> {
        errors.iter().map(|e| e.index).collect()
    }

    #[test]
    fn at_most_one_primary_contact() {
        let a = map(json!({ "name": "A", "is_primary": true }));
        let b = map(json!({ "name": "B", "is_primary": false }));
        let c = map(json!({ "name": "C", "is_primary": true }));
        let d = map(json!({ "name": "D" }));

        // 沒有或只有一位主要聯絡人
        assert!(check_lines(CUSTOMER_CONTACT, &[]).is_empty());
        assert!(check_lines(CUSTOMER_CONTACT, &[(0, &b), (1, &d)]).is_empty());
        assert!(check_lines(CUSTOMER_CONTACT, &[(0, &a), (1, &b), (2, &d)]).is_empty());

        // 兩位：指出第二列（index 為 request 陣列中的位置，不是過濾後的位置）
        let errors = check_lines(CUSTOMER_CONTACT, &[(0, &a), (1, &b), (3, &c)]);
        assert_eq!(line_indexes(&errors), [Some(3)]);
        for e in &errors {
            assert_eq!(e.detail_key, CUSTOMER_CONTACT);
            assert_eq!(e.field_key.as_deref(), Some(IS_PRIMARY));
            assert_eq!(e.message, MSG_CUSTOMER_PRIMARY);
        }

        // 三位：第二列起都指出
        let errors = check_lines(SUPPLIER_CONTACT, &[(0, &a), (1, &c), (2, &a)]);
        assert_eq!(line_indexes(&errors), [Some(1), Some(2)]);
        assert_eq!(errors[0].message, MSG_SUPPLIER_PRIMARY);
    }

    #[test]
    fn other_details_are_not_checked_for_primary() {
        let a = map(json!({ "is_primary": true }));
        assert!(check_lines("parts", &[(0, &a), (1, &a)]).is_empty());
    }

    #[test]
    fn category_cycle_includes_self_and_descendants() {
        let (root, child, grandchild, other) = (new_id(), new_id(), new_id(), new_id());
        // 指向自己
        assert!(creates_cycle(root, &[root]));
        // root 的上層設為孫分類：孫分類往上為 grandchild → child → root
        assert!(creates_cycle(root, &[grandchild, child, root]));
        // 指向其他分支
        assert!(!creates_cycle(child, &[other]));
        assert!(!creates_cycle(grandchild, &[root]));
    }

    #[test]
    fn delete_is_blocked_while_in_use() {
        assert!(delete_blocked(UNIT, Usage::default()).is_none());

        let err = delete_blocked(
            UNIT,
            Usage {
                products: 3,
                ..Usage::default()
            },
        )
        .unwrap();
        let api: ApiError = err.into();
        assert_eq!(api.message, "此單位還有 3 筆商品使用中，無法刪除");
        assert_eq!(
            api.details,
            Some(json!({ "fields": [], "in_use_count": 3 }))
        );

        let api: ApiError = delete_blocked(
            PRODUCT_CATEGORY,
            Usage {
                products: 2,
                children: 1,
                ..Usage::default()
            },
        )
        .unwrap()
        .into();
        assert_eq!(
            api.message,
            "此分類還有 2 筆商品使用中、1 個子分類，無法刪除"
        );
        assert_eq!(
            api.details,
            Some(json!({ "fields": [], "in_use_count": 2, "child_count": 1 }))
        );

        let api: ApiError = delete_blocked(
            PRODUCT_CATEGORY,
            Usage {
                children: 4,
                ..Usage::default()
            },
        )
        .unwrap()
        .into();
        assert_eq!(api.message, "此分類還有 4 個子分類，無法刪除");
        assert_eq!(api.details, Some(json!({ "fields": [], "child_count": 4 })));
    }

    #[test]
    fn product_delete_is_blocked_by_stock_transactions() {
        assert!(delete_blocked(PRODUCT, Usage::default()).is_none());
        let api: ApiError = delete_blocked(
            PRODUCT,
            Usage {
                transactions: 12,
                ..Usage::default()
            },
        )
        .unwrap()
        .into();
        assert_eq!(api.message, MSG_PRODUCT_DELETE_STOCK);
        assert_eq!(
            api.details,
            Some(json!({ "fields": [], "transaction_count": 12 }))
        );
    }
}

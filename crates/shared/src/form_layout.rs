//! 表單版面（`docs/architecture.md` §7.7、§8.5 `form_layouts`、§9.4、§10）。
//!
//! 管理者可以自訂每個資料類別（主檔、單據）的表單畫面：把欄位分成多個區塊、每個區塊 1–4 欄、
//! 調整欄位寬度與順序，並移動系統區塊（明細表、挑選出貨單等）的位置。每個資料類別最多一個版面，
//! 只有全域版面。
//!
//! | 端點 | request | 成功回應 |
//! |---|---|---|
//! | `GET /api/v1/entities/{key}/form-layout` | | `200` [`FormLayout`] |
//! | `PUT /api/v1/entities/{key}/form-layout` | [`SaveFormLayoutRequest`] | `200` [`FormLayout`] |
//! | `DELETE /api/v1/entities/{key}/form-layout?version=N` | [`DeleteFormLayoutQuery`] | `200` [`FormLayout`]（還原後自動產生的版面） |
//!
//! 權限：讀取需要 `record.read`；修改、還原需要 `schema.manage`。
//!
//! 錯誤：
//!
//! - 資料類別不存在、已停用或已刪除、是明細子表 → `404 NOT_FOUND`；`DELETE` 時沒有設定版面 → `404`
//! - 沒有權限 → `403 FORBIDDEN`
//! - `config` 不合法 → `422 VALIDATION`，`details.field` 為路徑，例如
//!   `config.sections[1].items[3].field`
//! - `version` 不符，或第一次建立（`version` 為 `null`）時已有版面 → `409 CONFLICT`（§9.4）
//!
//! # 讀取時的處理
//!
//! `GET` 與寫入後回傳的 `config` 都已正規化（§7.7「讀取時的處理」）：略過已停用或不存在的欄位、
//! 明細子表與不再適用的系統區塊；未放進版面的有效、顯示欄位附加到 [`OTHER_SECTION_KEY`] 區塊；
//! 未放進版面的系統區塊依 [`BlockPlacement`] 插入第一個區塊的最前面，或附加到
//! [`BLOCKS_SECTION_KEY`] 區塊。沒有設定版面時依欄位定義自動產生（`is_default = true`）。

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::Id;

/// 一個區塊最多的欄數。
pub const MAX_COLUMNS: i32 = 4;
/// 一個版面最多的區塊數。
pub const MAX_SECTIONS: usize = 20;
/// 一個版面所有區塊的項目合計上限。
pub const MAX_ITEMS: usize = 300;
/// 區塊標題的長度上限（字元）。
pub const MAX_SECTION_TITLE_CHARS: usize = 50;
/// 區塊代碼的長度上限（字元）。
pub const MAX_SECTION_KEY_CHARS: usize = 50;

/// 自動產生（沒有設定版面）時唯一的區塊代碼。
pub const MAIN_SECTION_KEY: &str = "main";
/// 讀取時自動附加未放進版面的欄位的區塊代碼（保留代碼，寫入時改名）。
pub const OTHER_SECTION_KEY: &str = "_other";
/// [`OTHER_SECTION_KEY`] 區塊的標題。
pub const OTHER_SECTION_TITLE: &str = "其他欄位";
/// 讀取時自動附加未放進版面的系統區塊的區塊代碼（保留代碼，寫入時改名）。
pub const BLOCKS_SECTION_KEY: &str = "_blocks";

/// 系統區塊：單號。
pub const BLOCK_RECORD_NO: &str = "record_no";
/// 系統區塊：銷項發票的「選擇出貨單」。
pub const BLOCK_SHIPMENT_PICKER: &str = "shipment_picker";
/// 系統區塊：進項憑證的「選擇進貨單」。
pub const BLOCK_RECEIPT_PICKER: &str = "receipt_picker";
/// 系統區塊：銷貨折讓、進貨折讓的「可折讓金額」。
pub const BLOCK_ALLOWANCE_AVAILABLE: &str = "allowance_available";
/// 系統區塊：出貨單、銷貨退回的已用批號表。
pub const BLOCK_USED_LOTS: &str = "used_lots";
/// 明細表系統區塊代碼的前綴（`lines:<明細子表代碼>`）。
pub const LINES_BLOCK_PREFIX: &str = "lines:";

/// 明細子表 `detail_key` 的明細表系統區塊代碼。
pub fn lines_block(detail_key: &str) -> String {
    format!("{LINES_BLOCK_PREFIX}{detail_key}")
}

/// 系統區塊沒有放進版面時自動附加的位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum BlockPlacement {
    /// 欄位之前：插入第一個區塊的最前面。
    Top,
    /// 欄位之後：附加到最後的 [`BLOCKS_SECTION_KEY`] 區塊。
    Bottom,
}

/// 資料類別適用的系統區塊（不含 `lines:*`，明細表依資料類別目前有效的明細子表決定），
/// 依自動產生時的順序：`(代碼, 位置)`。
///
/// [`BlockPlacement::Bottom`] 的區塊與明細表的相對順序見 [`system_blocks`]。
pub fn fixed_blocks(entity_key: &str) -> Vec<(&'static str, BlockPlacement)> {
    let mut blocks = Vec::new();
    if matches!(entity_key, "sales_allowance" | "purchase_allowance") {
        blocks.push((BLOCK_ALLOWANCE_AVAILABLE, BlockPlacement::Top));
    }
    blocks.push((BLOCK_RECORD_NO, BlockPlacement::Top));
    match entity_key {
        "sales_invoice" => blocks.push((BLOCK_SHIPMENT_PICKER, BlockPlacement::Bottom)),
        "purchase_invoice" => blocks.push((BLOCK_RECEIPT_PICKER, BlockPlacement::Bottom)),
        _ => {}
    }
    blocks
}

/// 資料類別適用的所有系統區塊，依自動產生時的順序（與 M8 之前的 `RecordFormPage` 相同）：
/// 可折讓金額 → 單號 →（欄位）→ 選擇出貨單／進貨單 → 各明細表 → 已用批號表。
///
/// `detail_keys` 為有效的明細子表代碼，依明細子表的 `sort_order`。
pub fn system_blocks(entity_key: &str, detail_keys: &[&str]) -> Vec<(String, BlockPlacement)> {
    let mut blocks: Vec<(String, BlockPlacement)> = fixed_blocks(entity_key)
        .into_iter()
        .map(|(code, placement)| (code.to_owned(), placement))
        .collect();
    blocks.extend(
        detail_keys
            .iter()
            .map(|key| (lines_block(key), BlockPlacement::Bottom)),
    );
    if matches!(entity_key, "shipment" | "sales_return") {
        blocks.push((BLOCK_USED_LOTS.to_owned(), BlockPlacement::Bottom));
    }
    blocks
}

/// 區塊內的一個項目。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum FormItem {
    /// 主表的欄位。
    Field {
        /// 欄位代碼。
        field: String,
        /// 佔用的欄數（1 到所在區塊的 `columns`），省略時為 1。
        #[serde(default = "one")]
        span: i32,
    },
    /// 空白佔位。
    Spacer {
        /// 佔用的欄數（1 到所在區塊的 `columns`），省略時為 1。
        #[serde(default = "one")]
        span: i32,
    },
    /// 系統區塊，佔滿整列（`record_no` 與欄位相同佔一格）。
    Block {
        /// 系統區塊代碼，例如 `record_no`、`lines:goods_receipt_line`。
        block: String,
    },
}

fn one() -> i32 {
    1
}

/// 一個區塊。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct FormSection {
    /// 版面內不可重複；`^[a-z][a-z0-9_]{0,49}$`（[`OTHER_SECTION_KEY`]、[`BLOCKS_SECTION_KEY`]
    /// 寫入時改名）。
    pub key: String,
    /// 標題；空字串代表不顯示標題。
    #[serde(default)]
    pub title: String,
    /// 欄數（1–[`MAX_COLUMNS`]）。
    pub columns: i32,
    /// 是否可以收折；為 `true` 時 `title` 不可為空。
    #[serde(default)]
    pub collapsible: bool,
    /// 開啟表單時是否預設收折；只有 `collapsible` 時可以為 `true`。
    #[serde(default)]
    pub collapsed: bool,
    /// 項目，依顯示順序（由左而右、由上而下）。
    #[serde(default)]
    pub items: Vec<FormItem>,
}

/// 版面設定（`form_layouts.config`）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct FormLayoutConfig {
    /// 區塊，依由上而下的顯示順序。
    pub sections: Vec<FormSection>,
}

/// 資料類別的表單版面（`GET`、`PUT`、`DELETE` 的回應）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct FormLayout {
    /// 版面 id；自動產生時為 `null`。
    pub id: Option<Id>,
    /// 樂觀鎖定版本號（§9.4）；自動產生時為 `null`。
    pub version: Option<i32>,
    /// `true`：沒有設定版面，依欄位定義自動產生。
    pub is_default: bool,
    /// 正規化後的版面。
    pub config: FormLayoutConfig,
}

/// `PUT /api/v1/entities/{key}/form-layout` 的 request。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct SaveFormLayoutRequest {
    /// 讀取時取得的版本號；第一次建立（目前是自動產生的版面）時為 `null`。不符時 `409 CONFLICT`。
    pub version: Option<i32>,
    pub config: FormLayoutConfig,
}

/// `DELETE /api/v1/entities/{key}/form-layout` 的 query string。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct DeleteFormLayoutQuery {
    /// 讀取時取得的版本號；不符時回傳 `409 CONFLICT`。
    pub version: i32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn config_json_shape() {
        let config = FormLayoutConfig {
            sections: vec![FormSection {
                key: "basic".into(),
                title: "基本資料".into(),
                columns: 2,
                collapsible: false,
                collapsed: false,
                items: vec![
                    FormItem::Block {
                        block: "record_no".into(),
                    },
                    FormItem::Field {
                        field: "remark".into(),
                        span: 2,
                    },
                    FormItem::Spacer { span: 1 },
                ],
            }],
        };
        let v = serde_json::to_value(&config).unwrap();
        assert_eq!(
            v,
            json!({ "sections": [{
                "key": "basic", "title": "基本資料", "columns": 2,
                "collapsible": false, "collapsed": false,
                "items": [
                    { "type": "block", "block": "record_no" },
                    { "type": "field", "field": "remark", "span": 2 },
                    { "type": "spacer", "span": 1 },
                ],
            }]})
        );
        assert_eq!(
            serde_json::from_value::<FormLayoutConfig>(v).unwrap(),
            config
        );
    }

    #[test]
    fn optional_properties_have_defaults() {
        let section: FormSection = serde_json::from_value(json!({
            "key": "a", "columns": 1,
            "items": [{ "type": "field", "field": "x" }, { "type": "spacer" }],
        }))
        .unwrap();
        assert_eq!(section.title, "");
        assert!(!section.collapsible && !section.collapsed);
        assert_eq!(
            section.items,
            vec![
                FormItem::Field {
                    field: "x".into(),
                    span: 1
                },
                FormItem::Spacer { span: 1 },
            ]
        );
    }

    #[test]
    fn unknown_properties_are_rejected() {
        for value in [
            json!({ "sections": [], "extra": 1 }),
            json!({ "sections": [{ "key": "a", "columns": 1, "width": 3 }] }),
            json!({ "sections": [{ "key": "a", "columns": 1,
                     "items": [{ "type": "block", "block": "record_no", "span": 2 }] }] }),
            json!({ "sections": [{ "key": "a", "columns": 1,
                     "items": [{ "type": "tab" }] }] }),
        ] {
            assert!(
                serde_json::from_value::<FormLayoutConfig>(value.clone()).is_err(),
                "{value}"
            );
        }
    }

    #[test]
    fn system_blocks_follow_current_form_order() {
        let codes = |key: &str, details: &[&str]| -> Vec<String> {
            system_blocks(key, details)
                .into_iter()
                .map(|(code, _)| code)
                .collect()
        };
        assert_eq!(
            codes("customer", &["customer_contact"]),
            ["record_no", "lines:customer_contact"]
        );
        assert_eq!(
            codes("sales_invoice", &["sales_invoice_line"]),
            ["record_no", "shipment_picker", "lines:sales_invoice_line"]
        );
        assert_eq!(
            codes("sales_allowance", &["sales_allowance_line"]),
            [
                "allowance_available",
                "record_no",
                "lines:sales_allowance_line"
            ]
        );
        assert_eq!(
            codes("shipment", &["shipment_line"]),
            ["record_no", "lines:shipment_line", "used_lots"]
        );
        assert_eq!(
            system_blocks("purchase_allowance", &[])[0].1,
            BlockPlacement::Top
        );
        assert_eq!(
            system_blocks("purchase_invoice", &[])[1],
            (BLOCK_RECEIPT_PICKER.to_owned(), BlockPlacement::Bottom)
        );
    }
}

//! 銷貨模組的專屬端點型別（`docs/architecture.md` §8.9）。
//!
//! # 銷貨用商品選擇 API（§8.9「銷貨用商品選擇 API」，M11）
//!
//! `GET /api/v1/sales/product-lookup?customer_id=&q=&limit=`，需要 `record.read`，回應
//! [`SalesProductLookupItem`] 陣列（不分頁）。訂單、出貨單的明細選商品時使用：可以用這位客戶的
//! 客戶品名搜尋，並一次取得要帶入的品名。
//!
//! - `customer_id`：可省略（空字串視為未指定）；不是 uuid → `422 VALIDATION`
//!   （`details.field = customer_id`）；客戶不存在或已刪除 → `404 NOT_FOUND`
//! - `q`、`limit`：與通用 `lookup`（[`crate::record::LookupQuery`]）相同
//! - 範圍：未刪除、啟用中的商品；客戶品名只用這位客戶未刪除的那筆
//! - 比對：每個詞都要出現在「商品標題 + 客戶品名」中（不分大小寫）
//! - 排序：每個詞都出現在客戶品名中的商品在前（`q` 空白時為所有有客戶品名的商品），其他在後；
//!   同一組內由新到舊
//!
//! # 批次取得明細品名 API（§8.9，M12）
//!
//! 見 [`product_names_param`]、[`SalesProductName`]。

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::Id;

/// [`SalesProductLookupItem`] 的參數名稱（`q`、`limit` 見 [`crate::record::lookup_param`]）。
pub mod product_lookup_param {
    /// 客戶 id。
    pub const CUSTOMER_ID: &str = "customer_id";
}

/// `GET /api/v1/sales/product-lookup` 的 query string（桌面程式送出時使用；伺服器以參數清單解析）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct SalesProductLookupQuery {
    /// 客戶 id；省略時與一般商品搜尋相同。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub customer_id: Option<Id>,
    /// 關鍵字（以空白分隔，每個詞都要出現）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    /// 筆數上限（預設 20、上限 50）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// 銷貨用商品選擇 API 的一筆結果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct SalesProductLookupItem {
    pub id: Id,
    /// 商品標題（與通用 `lookup` 相同，預設 `{code} {name}`）。
    pub title: String,
    pub code: String,
    pub name: String,
    pub spec: Option<String>,
    /// `goods` / `service`。
    pub product_type: String,
    /// 預設售價（未稅，4 位小數）；沒有設定時為 `null`。
    #[serde(with = "crate::decimal::serde_str_option")]
    #[specta(type = Option<String>)]
    pub default_sale_price: Option<Decimal>,
    /// 這位客戶的客戶品名；沒有設定或沒帶 `customer_id` 時為 `null`。
    pub customer_product_name: Option<String>,
    /// 依帶入順序決定的品名（`customer_product_name` ?? `name`），選定後帶入明細的 `product_name`。
    pub product_name: String,
}

/// [`SalesProductName`] 的參數名稱與上限（§8.9「批次取得明細品名 API」，M12）。
///
/// `GET /api/v1/sales/product-names?customer_id=&product_id=&product_id=…`，需要 `record.read`，回應
/// [`SalesProductName`] 陣列（依 `product_id` 第一次出現的順序）。
///
/// - `customer_id`：必填；省略、空字串或不是 uuid → `422 VALIDATION`（`details.field = customer_id`）；
///   客戶不存在或已刪除 → `404 NOT_FOUND`
/// - `product_id`：可重複，1–[`MAX_PRODUCTS`] 個（以參數個數計）；沒有帶、超過上限、不是 uuid →
///   `422 VALIDATION`（`details.field = product_id`）
/// - 範圍：未刪除的商品（含停用）；不存在、已刪除的略過，重複的只回傳一筆
pub mod product_names_param {
    /// 客戶 id。
    pub const CUSTOMER_ID: &str = "customer_id";
    /// 商品 id（可重複）。
    pub const PRODUCT_ID: &str = "product_id";
    /// 一次最多幾個 `product_id`。
    pub const MAX_PRODUCTS: usize = 200;
}

/// 批次取得明細品名 API 的一筆結果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct SalesProductName {
    pub product_id: Id,
    /// 這位客戶的客戶品名；沒有設定時為 `null`。
    pub customer_product_name: Option<String>,
    /// 依帶入順序決定的品名（`customer_product_name` ?? 商品的 `name`）。
    pub product_name: String,
}

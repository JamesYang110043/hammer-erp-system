//! 自訂資料類別的資料（`docs/architecture.md` §7.1、§8.5 `custom_records`、§10）。
//!
//! 自訂資料類別（`storage = custom_records`）共用通用端點：
//!
//! | 端點 | request | 成功回應 |
//! |---|---|---|
//! | `GET /api/v1/entities/{key}/records` | [`RecordListQuery`]（query string） | `200` [`Page`](crate::pagination::Page)`<`[`Record`]`>` |
//! | `GET /api/v1/entities/{key}/records/{id}` | | `200` [`Record`] |
//! | `POST /api/v1/entities/{key}/records` | [`CreateRecordRequest`] | `201` [`Record`] |
//! | `PATCH /api/v1/entities/{key}/records/{id}` | [`UpdateRecordRequest`] | `200` [`Record`] |
//! | `DELETE /api/v1/entities/{key}/records/{id}?version=N` | [`DeleteRecordQuery`] | `204 No Content` |
//! | `GET /api/v1/entities/{key}/records/lookup?q=&limit=` | [`LookupQuery`]（query string） | `200` [`LookupItem`]`[]` |
//!
//! 讀取（含 `lookup`）需要 `record.read`，寫入需要 `record.write`。`data` 的格式見
//! [`crate::schema::value`]。
//!
//! # 單號（`record_no`）
//!
//! 資料類別有設定單號規則（`numbering_pattern`）時，新增資料由伺服器依規則產生單號
//! （§8.4 `number_sequences`），回應的 [`Record::record_no`] 為產生的單號；沒有規則時為 `null`。
//!
//! - 單號**不可由客戶端指定或修改**：新增、修改的 request 中帶了 `record_no` 一律忽略；
//!   `data` 內帶 `record_no` 鍵則視為未定義的欄位（`422 VALIDATION`）
//! - [`RECORD_NO`] 為保留的代碼，不可作為欄位代碼
//! - 修改規則只影響之後新增的資料，既有的單號不變；已刪除資料的單號不會再被使用
//! - 修改規則後產生的單號與既有的單號重複時，新增失敗（`422 VALIDATION`），需要調整規則
//!
//! # 標題（`title`）
//!
//! 列表與單筆回應的 [`Record::title`] 是「這筆資料叫什麼」的顯示文字，供關聯欄位、選擇器、
//! 操作紀錄使用，由伺服器依資料類別的 `title_template` 產生（§8.5 `entity_definitions`）：
//!
//! - 範本可引用 `{record_no}` 與本資料類別的欄位 `{field_key}`，例如 `{record_no} {name}`；
//!   值依欄位型態格式化（選項顯示名稱、勾選顯示 是／否、日期時間為台北時間），沒有值時為空字串，
//!   已停用的欄位視為沒有值
//! - 沒有範本時依序使用：單號 → 排序最前面、有效的 `text` 欄位的值 → id 前 8 碼
//!
//! # 關聯欄位（`relation`）
//!
//! `relation` 欄位的值是目標資料類別（欄位定義的 `relation_entity_id`）中一筆資料的 id。
//!
//! - 寫入：值必須是目標資料類別中**未刪除**的資料 id；不存在、已刪除、屬於其他資料類別 →
//!   `422 VALIDATION`，`details.fields` 指出欄位。修改資料時只檢查有變動的關聯欄位
//! - 目標資料之後被刪除：允許刪除，關聯欄位保留原值
//! - 回應：[`Record::relations`] 列出 `data` 中每個有值的關聯欄位所指的資料
//!   `{ "<field_key>": { "id", "title", "deleted" } }`（[`RelationRef`]）。`title` 依目標資料類別的
//!   標題規則產生；目標已刪除時 `deleted` 為 `true`，`title` 仍為刪除前的標題
//! - 篩選只支援 `eq`（值為資料 id）與 `empty`；不能排序
//! - 選擇器：`GET /api/v1/entities/{key}/records/lookup` 搜尋 `{key}` 的資料，見 [`LookupQuery`]
//!
//! # 附件欄位（`file`）
//!
//! `file` 欄位的值是附件 id，上傳與綁定的規則見 [`crate::attachment`]。
//!
//! - 回應：[`Record::attachments`] 列出 `data` 中每個有值的 `file` 欄位所指的附件
//!   `{ "<field_key>": { "id", "file_name", "mime_type", "size_bytes", "sha256", "created_at" } }`
//!   （[`Attachment`]），供顯示檔名、大小；找不到附件（已標記刪除）時不列出
//!
//! # 公式欄位（`formula`）
//!
//! 公式欄位的值由伺服器在存檔時計算（architecture.md §7.6），存值格式依欄位的
//! `formula_result_type`。
//!
//! - 新增與修改（含主表與明細一起存檔）時，在同一個 transaction 內計算所有有效的公式欄位：先算每一列
//!   明細的公式，再算主表的公式（主表的子表彙總使用明細公式的結果）；修改時所有公式都重算，包含
//!   request 沒有帶到的明細子表
//! - request 的 `data` 帶了公式欄位的值時一律忽略（不回傳錯誤）
//! - 關聯引用讀取目標資料目前的值（目標已刪除時仍讀取）；目標之後被修改時，引用它的資料**不會**
//!   自動更新，要等該筆資料再次存檔
//! - `TODAY()` 為存檔時 Asia/Taipei 的日期
//! - 計算失敗（除以零、型態不符等）時存檔仍成功：該欄位在 `data` 中沒有值，
//!   [`Record::formula_errors`] 記錄原因 `{ "<field_key>": "除以零" }`
//! - 篩選與排序依 `formula_result_type` 支援對應的運算子（見 [`FilterOp`]）
//! - 公式欄位的值變動記在操作紀錄的 diff
//!
//! # 明細子表（`lines`）
//!
//! 明細子表是 `kind = detail` 的資料類別（例如維修單的「更換零件」），屬於一個主表
//! （`parent_entity_id`，`master` 或 `document`）。明細的資料只能透過主表存取：
//! `/api/v1/entities/{明細子表代碼}/records`（含 `lookup`）一律回傳 `404 NOT_FOUND`。
//!
//! - 新增、修改的 request 以 `lines: { "<明細子表代碼>": [ { "id"?, "data": { … } } ] }` 帶明細
//!   （[`LineInput`]）
//!   - 新增：所有明細與主表一起建立；明細不可帶 `id`
//!   - 修改：有帶到的明細子表「整組取代」——有 `id` 的更新（`data` 只合併有帶到的欄位）、沒有 `id`
//!     的新增、目前有但沒列出的刪除（軟刪除）；沒帶到的明細子表不變。`id` 必須是這筆主表在該明細子表
//!     的未刪除明細，同一個 `id` 不可出現兩次
//! - 明細的順序為陣列中的位置（`sort_order`，從 0 開始）；修改時調整陣列順序即可重新排序
//! - 主表與明細在同一個 transaction 內存檔，任一筆不合法就整筆不寫入
//! - 樂觀鎖定只看主表的 `version`；明細有任何變動（含順序）時主表的 `version` 也遞增
//! - 回應：單筆查詢與新增、修改的回應 [`Record::lines`] 包含主表所有有效的明細子表，每一列明細的
//!   格式同 [`Record`]（`record_no`、`lines` 為 `null`）；列表不回傳明細
//! - 刪除主表時明細一起刪除（軟刪除）
//! - 操作紀錄：明細的變動記在主表那筆紀錄的 diff，鍵為 `lines.<明細子表代碼>`，值為該明細子表
//!   所有明細的 `[{ "id", "data" }]`（依順序）；只有變動的明細子表會出現
//!
//! 錯誤：
//!
//! - 資料類別不存在、已停用或不是自訂資料類別；資料不存在或已刪除 → `404 NOT_FOUND`
//! - `data` 不符合欄位定義（含違反「不可重複」）→ `422 VALIDATION`，
//!   `details.fields` 為 `[{ "field_key", "message" }]`，列出所有出錯的欄位
//! - 明細不合法（欄位錯誤、明細子表不存在、`id` 不存在或重複）→ `422 VALIDATION`，`details` 為
//!   `{ "fields": [ … 主表的欄位錯誤 … ], "lines": [{ "detail_key", "index", "field_key", "message" }] }`：
//!   `index` 為該列在 request 陣列中的位置（從 0 開始），`field_key` 為出錯的欄位；
//!   不屬於某一列（明細子表不存在）時 `index` 為 `null`，不屬於某個欄位時 `field_key` 為 `null`
//! - 列表的篩選、排序參數不合法 → `422 VALIDATION`，`details.field` 為參數名稱（例如 `filter.qty.gte`）
//! - `page`、`page_size` 不是整數 → `400 BAD_REQUEST`
//! - `version` 不符 → `409 CONFLICT`（§9.4）
//!
//! # 列表的 query string
//!
//! | 參數 | 說明 |
//! |---|---|
//! | `page`、`page_size` | 分頁，見 [`crate::pagination`] |
//! | `sort` | 排序欄位代碼；前面加 `-` 代表由大到小，例如 `sort=-qty`。省略時依建立順序由新到舊 |
//! | `filter.<field_key>.<op>` | 篩選條件，運算子見 [`FilterOp`]，例如 `filter.qty.gte=10` |
//!
//! `sort` 與 `filter` 可以使用 [`RECORD_NO`] 代表單號：依文字排序（沒有單號的資料排在最後），
//! 篩選只支援 `eq`、`contains`，例如 `sort=-record_no`、`filter.record_no.contains=2611`。
//!
//! - 多個篩選條件須全部符合（AND）；同一個欄位的多個 `any` 合併為一個條件（符合任一個即可）
//! - 值為空字串的條件視為未指定
//! - 依欄位型態比較與排序（數值不會按字串排序）；沒有值的資料排在最後；
//!   次要排序為 `id`（方向與主要排序相同）
//! - 其他參數名稱忽略
//!
//! [`RecordListQuery::from_pairs`] 解析語法；欄位是否存在、運算子是否適用於該型態、
//! 值的格式由伺服器依欄位定義檢查。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use specta::Type;
use time::OffsetDateTime;

use crate::Id;
use crate::attachment::Attachment;
use crate::pagination::field as page_field;

/// 單號在列表排序、篩選中使用的代碼，也是保留的欄位代碼（不可作為 `field_key`）。
pub const RECORD_NO: &str = "record_no";

/// 一筆自訂資料類別的資料。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct Record {
    pub id: Id,
    /// 單號，由伺服器依資料類別的單號規則產生；沒有單號時為 `null`。
    pub record_no: Option<String>,
    /// 資料標題，由伺服器依資料類別的標題範本（`title_template`）產生，見模組說明。
    pub title: String,
    /// 所有欄位的值，鍵為 `field_key`，格式依 [`crate::schema::value`]。沒有值的欄位不會出現。
    #[specta(type = std::collections::HashMap<String, specta_typescript::Unknown>)]
    pub data: Map<String, Value>,
    /// 關聯欄位所指的資料，鍵為 `field_key`；只包含 `data` 中有值的 `relation` 欄位
    /// （見模組說明「關聯欄位」）。
    #[serde(default)]
    pub relations: BTreeMap<String, RelationRef>,
    /// 附件欄位所指的附件，鍵為 `field_key`；只包含 `data` 中有值、且附件未標記刪除的 `file` 欄位
    /// （見模組說明「附件欄位」）。
    #[serde(default)]
    pub attachments: BTreeMap<String, Attachment>,
    /// 公式欄位的計算錯誤，鍵為 `field_key`、值為錯誤原因（繁體中文）；只包含計算失敗的公式欄位
    /// （見模組說明「公式欄位」），沒有錯誤時為空物件。
    #[serde(default)]
    pub formula_errors: BTreeMap<String, String>,
    /// 明細，鍵為明細子表的代碼（見模組說明「明細子表」）。單筆查詢與新增、修改的回應包含主表所有
    /// 有效的明細子表（沒有明細時為空陣列），依 `sort_order` 排序；列表與明細本身為 `null`（不輸出）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<BTreeMap<String, Vec<Record>>>,
    /// 建立時間（RFC 3339）。
    #[serde(with = "time::serde::rfc3339")]
    #[specta(type = String)]
    pub created_at: OffsetDateTime,
    pub created_by: Option<Id>,
    /// 最後修改時間（RFC 3339）。
    #[serde(with = "time::serde::rfc3339")]
    #[specta(type = String)]
    pub updated_at: OffsetDateTime,
    pub updated_by: Option<Id>,
    /// 樂觀鎖定版本號（§9.4）。
    pub version: i32,
}

/// 關聯欄位所指的一筆資料（[`Record::relations`] 的值）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RelationRef {
    /// 目標資料的 id（與 `data` 中的值相同）。
    pub id: Id,
    /// 目標資料的標題；找不到目標資料時為空字串。
    pub title: String,
    /// 目標資料已刪除（或找不到）。
    pub deleted: bool,
}

/// `GET /api/v1/entities/{key}/records/lookup` 的 query string（選擇器搜尋）。
///
/// | 參數 | 說明 |
/// |---|---|
/// | `q` | 關鍵字，比對資料標題（[`Record::title`]）：以空白分隔的每個詞都要出現在標題中，不分大小寫。省略或空白時不篩選 |
/// | `limit` | 最多回傳的筆數，預設 [`LOOKUP_DEFAULT_LIMIT`]，上限 [`LOOKUP_MAX_LIMIT`]（超過時套用上限） |
///
/// 回應為 [`LookupItem`] 陣列，不含已刪除的資料，依建立順序由新到舊。
///
/// 錯誤：`limit` 不是整數 → `400 BAD_REQUEST`；小於 1 → `422 VALIDATION`（`details.field = limit`）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct LookupQuery {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

/// [`LookupQuery`] 的參數名稱。
pub mod lookup_param {
    /// 關鍵字。
    pub const Q: &str = "q";
    /// 筆數上限。
    pub const LIMIT: &str = "limit";
}

/// `lookup` 預設回傳的筆數。
pub const LOOKUP_DEFAULT_LIMIT: u32 = 20;
/// `lookup` 最多回傳的筆數。
pub const LOOKUP_MAX_LIMIT: u32 = 50;

impl LookupQuery {
    /// 由 query string 的 `(名稱, 值)` 解析；其他參數忽略，重複時以最後一個為準。
    ///
    /// `limit` 不是非負整數 → [`QueryParamError::NotInteger`]；為 0 →
    /// [`QueryParamError::Invalid`]。
    pub fn from_pairs<K, V>(pairs: &[(K, V)]) -> Result<Self, QueryParamError>
    where
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let mut query = Self::default();
        for (name, value) in pairs {
            match name.as_ref() {
                lookup_param::Q => query.q = Some(value.as_ref().to_owned()),
                lookup_param::LIMIT => {
                    let limit = parse_u32(lookup_param::LIMIT, value.as_ref())?;
                    if limit == 0 {
                        return Err(QueryParamError::Invalid {
                            param: lookup_param::LIMIT.to_owned(),
                            message: "limit 必須大於等於 1".to_owned(),
                        });
                    }
                    query.limit = Some(limit);
                }
                _ => {}
            }
        }
        Ok(query)
    }

    /// 實際套用的筆數上限。
    pub fn effective_limit(&self) -> u32 {
        self.limit
            .unwrap_or(LOOKUP_DEFAULT_LIMIT)
            .clamp(1, LOOKUP_MAX_LIMIT)
    }

    /// 關鍵字以空白分隔、轉成小寫後的詞；沒有關鍵字時為空。
    pub fn terms(&self) -> Vec<String> {
        self.q
            .as_deref()
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_lowercase)
            .collect()
    }
}

/// 標題是否符合 [`LookupQuery::terms`]：每個詞都出現在標題中（不分大小寫）。沒有詞時一律符合。
pub fn title_matches(title: &str, terms: &[String]) -> bool {
    if terms.is_empty() {
        return true;
    }
    let title = title.to_lowercase();
    terms.iter().all(|t| title.contains(t.as_str()))
}

/// 選擇器搜尋的一筆結果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct LookupItem {
    pub id: Id,
    /// 資料標題（[`Record::title`]）。
    pub title: String,
}

/// 新增資料的輸入。沒有帶到的欄位套用預設值。單號由伺服器產生，request 中的 `record_no` 會被忽略。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
pub struct CreateRecordRequest {
    #[serde(default)]
    #[specta(type = std::collections::HashMap<String, specta_typescript::Unknown>)]
    pub data: Map<String, Value>,
    /// 明細，鍵為明細子表的代碼；與主表一起建立（見模組說明「明細子表」）。明細不可帶 `id`。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub lines: BTreeMap<String, Vec<LineInput>>,
}

/// 一列明細的輸入（[`CreateRecordRequest::lines`]、[`UpdateRecordRequest::lines`]）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
pub struct LineInput {
    /// 既有明細的 id；新的明細不帶（或 `null`）。
    #[serde(default)]
    pub id: Option<Id>,
    /// 欄位的值，格式同主表的 `data`。新的明細沒有帶到的欄位套用預設值；既有的明細只合併有帶到的欄位，
    /// `null` 代表清空。
    #[serde(default)]
    #[specta(type = std::collections::HashMap<String, specta_typescript::Unknown>)]
    pub data: Map<String, Value>,
}

/// 修改資料的輸入。只合併有帶到的欄位，值為 `null` 代表清空。單號不可修改，`record_no` 會被忽略。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct UpdateRecordRequest {
    /// 讀取時取得的版本號；不符時回傳 `409 CONFLICT`。
    pub version: i32,
    #[serde(default)]
    #[specta(type = std::collections::HashMap<String, specta_typescript::Unknown>)]
    pub data: Map<String, Value>,
    /// 要修改的明細子表，鍵為明細子表的代碼。有帶到的明細子表以陣列「整組取代」：有 `id` 的更新、
    /// 沒有 `id` 的新增、沒列出的刪除；沒帶到的明細子表不變（見模組說明「明細子表」）。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub lines: BTreeMap<String, Vec<LineInput>>,
}

/// `DELETE /api/v1/entities/{key}/records/{id}` 的 query string。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct DeleteRecordQuery {
    /// 讀取時取得的版本號；不符時回傳 `409 CONFLICT`。
    pub version: i32,
}

/// 篩選運算子。query string 中為 snake_case 字串。
///
/// | 運算子 | 說明 | 適用型態 |
/// |---|---|---|
/// | `eq` | 等於 | `multi_select` 以外 |
/// | `contains` | 包含（不分大小寫） | `text`、`long_text` |
/// | `gt`、`gte`、`lt`、`lte` | 大於、大於等於、小於、小於等於 | `integer`、`decimal`、`money`、`date`、`datetime` |
/// | `empty` | 值為 `true`：沒有值；`false`：有值 | 全部 |
/// | `any` | 包含任一選項；可重複帶多個 | `multi_select` |
///
/// `formula` 欄位依它的 `formula_result_type` 套用上表。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum FilterOp {
    Eq,
    Contains,
    Gt,
    Gte,
    Lt,
    Lte,
    Empty,
    Any,
}

impl FilterOp {
    pub const ALL: &[FilterOp] = &[
        Self::Eq,
        Self::Contains,
        Self::Gt,
        Self::Gte,
        Self::Lt,
        Self::Lte,
        Self::Empty,
        Self::Any,
    ];

    /// query string 使用的字串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Eq => "eq",
            Self::Contains => "contains",
            Self::Gt => "gt",
            Self::Gte => "gte",
            Self::Lt => "lt",
            Self::Lte => "lte",
            Self::Empty => "empty",
            Self::Any => "any",
        }
    }

    /// 由字串轉換；不是已定義的運算子時回傳 `None`。
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|op| op.as_str() == s)
    }
}

/// 一個篩選條件：`filter.<field_key>.<op>=<value>`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RecordFilter {
    pub field_key: String,
    pub op: FilterOp,
    /// 依欄位型態的文字表示：數值 `12.5`、日期 `YYYY-MM-DD`、日期時間 RFC 3339、
    /// 勾選 `true` / `false`；`empty` 為 `true` / `false`。
    pub value: String,
}

impl RecordFilter {
    /// query string 的參數名稱，也是驗證錯誤 `details.field` 的值。
    pub fn param_name(&self) -> String {
        filter_param(&self.field_key, self.op.as_str())
    }
}

/// 排序：`sort=<field_key>` 或 `sort=-<field_key>`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RecordSort {
    pub field_key: String,
    /// `true`：由大到小。
    pub descending: bool,
}

/// [`RecordListQuery`] 的參數名稱。
pub mod query_param {
    /// 排序。
    pub const SORT: &str = "sort";
    /// 篩選參數的前綴：`filter.<field_key>.<op>`。
    pub const FILTER_PREFIX: &str = "filter.";
}

/// `GET /api/v1/entities/{key}/records` 的 query string（格式見模組說明）。
///
/// query string 的參數名稱是動態的，用 [`Self::from_pairs`] 解析、[`Self::to_pairs`] 產生。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RecordListQuery {
    /// 頁碼，從 1 開始（預設 1）。
    pub page: Option<u32>,
    /// 每頁筆數（預設 50，上限 200）。
    pub page_size: Option<u32>,
    pub sort: Option<RecordSort>,
    /// 依 query string 的順序。
    pub filters: Vec<RecordFilter>,
}

/// query string 不合法。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryParamError {
    /// `page`、`page_size` 不是整數 → `400 BAD_REQUEST`。
    NotInteger { param: &'static str },
    /// 參數格式錯誤 → `422 VALIDATION`，`details.field` 為 `param`。
    Invalid { param: String, message: String },
}

impl QueryParamError {
    /// 顯示給使用者的訊息。
    pub fn message(&self) -> String {
        match self {
            Self::NotInteger { param } => format!("{param} 必須是整數"),
            Self::Invalid { message, .. } => message.clone(),
        }
    }
}

impl RecordListQuery {
    /// 由 query string 的 `(名稱, 值)` 解析（值已經過 URL 解碼）。
    ///
    /// 只檢查語法：`page`、`page_size` 必須是非負整數；`sort` 不可為空；
    /// `filter.` 開頭的參數必須是 `filter.<field_key>.<op>` 且 `op` 是 [`FilterOp`]。
    /// 值為空字串的 `sort` 與篩選條件視為未指定。重複的 `page`、`page_size`、`sort` 以最後一個為準。
    pub fn from_pairs<K, V>(pairs: &[(K, V)]) -> Result<Self, QueryParamError>
    where
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let mut query = Self::default();
        for (name, value) in pairs {
            let (name, value) = (name.as_ref(), value.as_ref());
            match name {
                page_field::PAGE => query.page = Some(parse_u32(page_field::PAGE, value)?),
                page_field::PAGE_SIZE => {
                    query.page_size = Some(parse_u32(page_field::PAGE_SIZE, value)?);
                }
                query_param::SORT => {
                    let value = value.trim();
                    if value.is_empty() {
                        query.sort = None;
                        continue;
                    }
                    let (field_key, descending) = match value.strip_prefix('-') {
                        Some(key) => (key, true),
                        None => (value, false),
                    };
                    if field_key.is_empty() {
                        return Err(QueryParamError::Invalid {
                            param: query_param::SORT.to_owned(),
                            message: "排序欄位不可為空".to_owned(),
                        });
                    }
                    query.sort = Some(RecordSort {
                        field_key: field_key.to_owned(),
                        descending,
                    });
                }
                _ => {
                    let Some(rest) = name.strip_prefix(query_param::FILTER_PREFIX) else {
                        continue;
                    };
                    let invalid = |message: &str| QueryParamError::Invalid {
                        param: name.to_owned(),
                        message: message.to_owned(),
                    };
                    let (field_key, op) = rest
                        .rsplit_once('.')
                        .filter(|(key, _)| !key.is_empty())
                        .ok_or_else(|| {
                            invalid("篩選參數的格式必須是 filter.<欄位代碼>.<運算子>")
                        })?;
                    let op = FilterOp::parse(op)
                        .ok_or_else(|| invalid(&format!("不支援的篩選運算子：{op}")))?;
                    if value.trim().is_empty() {
                        continue;
                    }
                    query.filters.push(RecordFilter {
                        field_key: field_key.to_owned(),
                        op,
                        value: value.to_owned(),
                    });
                }
            }
        }
        Ok(query)
    }

    /// 轉成 query string 的 `(名稱, 值)`，順序為 `page`、`page_size`、`sort`、篩選條件。
    /// 與 [`Self::from_pairs`] 互為反向。
    pub fn to_pairs(&self) -> Vec<(String, String)> {
        let mut pairs = Vec::new();
        if let Some(page) = self.page {
            pairs.push((page_field::PAGE.to_owned(), page.to_string()));
        }
        if let Some(size) = self.page_size {
            pairs.push((page_field::PAGE_SIZE.to_owned(), size.to_string()));
        }
        if let Some(sort) = &self.sort {
            let prefix = if sort.descending { "-" } else { "" };
            pairs.push((
                query_param::SORT.to_owned(),
                format!("{prefix}{}", sort.field_key),
            ));
        }
        for f in &self.filters {
            pairs.push((f.param_name(), f.value.clone()));
        }
        pairs
    }
}

/// 篩選參數名稱：`filter.<field_key>.<op>`。
pub fn filter_param(field_key: &str, op: &str) -> String {
    format!("{}{field_key}.{op}", query_param::FILTER_PREFIX)
}

fn parse_u32(param: &'static str, value: &str) -> Result<u32, QueryParamError> {
    value
        .parse()
        .map_err(|_| QueryParamError::NotInteger { param })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(pairs: &[(&str, &str)]) -> Result<RecordListQuery, QueryParamError> {
        RecordListQuery::from_pairs(pairs)
    }

    fn filter(field_key: &str, op: FilterOp, value: &str) -> RecordFilter {
        RecordFilter {
            field_key: field_key.into(),
            op,
            value: value.into(),
        }
    }

    #[test]
    fn empty_query() {
        assert_eq!(parse(&[]).unwrap(), RecordListQuery::default());
    }

    #[test]
    fn parses_page_sort_and_filters() {
        let q = parse(&[
            ("page", "2"),
            ("page_size", "20"),
            ("sort", "-qty"),
            ("filter.qty.gte", "10"),
            ("filter.tags.any", "a"),
            ("filter.tags.any", "b"),
            ("filter.note.empty", "true"),
            ("unknown", "x"),
        ])
        .unwrap();
        assert_eq!(
            q,
            RecordListQuery {
                page: Some(2),
                page_size: Some(20),
                sort: Some(RecordSort {
                    field_key: "qty".into(),
                    descending: true,
                }),
                filters: vec![
                    filter("qty", FilterOp::Gte, "10"),
                    filter("tags", FilterOp::Any, "a"),
                    filter("tags", FilterOp::Any, "b"),
                    filter("note", FilterOp::Empty, "true"),
                ],
            }
        );
        assert_eq!(
            parse(&[("sort", "name")]).unwrap().sort,
            Some(RecordSort {
                field_key: "name".into(),
                descending: false,
            })
        );
    }

    #[test]
    fn blank_values_are_ignored() {
        let q = parse(&[("sort", " "), ("filter.name.eq", ""), ("filter.x.eq", "  ")]).unwrap();
        assert_eq!(q, RecordListQuery::default());
    }

    #[test]
    fn non_integer_page_is_bad_request() {
        assert_eq!(
            parse(&[("page", "abc")]).unwrap_err(),
            QueryParamError::NotInteger { param: "page" }
        );
        assert_eq!(
            parse(&[("page_size", "-1")]).unwrap_err(),
            QueryParamError::NotInteger { param: "page_size" }
        );
        // 0 不是語法錯誤，由 PageRequest 回傳 422
        assert_eq!(parse(&[("page", "0")]).unwrap().page, Some(0));
    }

    #[test]
    fn malformed_filter_params_are_invalid() {
        for name in ["filter.qty", "filter..eq", "filter.", "filter.qty.between"] {
            match parse(&[(name, "1")]).unwrap_err() {
                QueryParamError::Invalid { param, .. } => assert_eq!(param, name),
                other => panic!("{name}：{other:?}"),
            }
        }
        match parse(&[("sort", "-")]).unwrap_err() {
            QueryParamError::Invalid { param, .. } => assert_eq!(param, "sort"),
            other => panic!("{other:?}"),
        }
        // 運算子大小寫有區別
        assert!(parse(&[("filter.qty.EQ", "1")]).is_err());
    }

    #[test]
    fn pairs_round_trip() {
        let q = RecordListQuery {
            page: Some(3),
            page_size: Some(10),
            sort: Some(RecordSort {
                field_key: "due_date".into(),
                descending: true,
            }),
            filters: vec![
                filter("name", FilterOp::Contains, "王"),
                filter("tags", FilterOp::Any, "a"),
            ],
        };
        let pairs = q.to_pairs();
        assert_eq!(
            pairs,
            [
                ("page".to_owned(), "3".to_owned()),
                ("page_size".to_owned(), "10".to_owned()),
                ("sort".to_owned(), "-due_date".to_owned()),
                ("filter.name.contains".to_owned(), "王".to_owned()),
                ("filter.tags.any".to_owned(), "a".to_owned()),
            ]
        );
        assert_eq!(RecordListQuery::from_pairs(&pairs).unwrap(), q);
    }

    #[test]
    fn filter_op_strings() {
        for op in FilterOp::ALL {
            assert_eq!(serde_json::to_value(op).unwrap(), json!(op.as_str()));
            assert_eq!(FilterOp::parse(op.as_str()), Some(*op));
        }
        assert_eq!(FilterOp::parse("between"), None);
    }

    #[test]
    fn lookup_query_parsing() {
        assert_eq!(
            LookupQuery::from_pairs::<&str, &str>(&[]).unwrap(),
            LookupQuery::default()
        );
        let q =
            LookupQuery::from_pairs(&[("q", " 台北  Pump "), ("limit", "5"), ("x", "1")]).unwrap();
        assert_eq!(q.limit, Some(5));
        assert_eq!(q.terms(), ["台北", "pump"]);
        assert_eq!(q.effective_limit(), 5);
        assert_eq!(
            LookupQuery::default().effective_limit(),
            LOOKUP_DEFAULT_LIMIT
        );
        assert_eq!(
            LookupQuery::from_pairs(&[("limit", "999")])
                .unwrap()
                .effective_limit(),
            LOOKUP_MAX_LIMIT
        );
        assert!(
            LookupQuery::from_pairs(&[("q", "   ")])
                .unwrap()
                .terms()
                .is_empty()
        );
        assert_eq!(
            LookupQuery::from_pairs(&[("limit", "abc")]).unwrap_err(),
            QueryParamError::NotInteger { param: "limit" }
        );
        match LookupQuery::from_pairs(&[("limit", "0")]).unwrap_err() {
            QueryParamError::Invalid { param, .. } => assert_eq!(param, "limit"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn title_matching() {
        let terms = |q: &str| {
            LookupQuery {
                q: Some(q.into()),
                limit: None,
            }
            .terms()
        };
        assert!(title_matches("EQ-001 台北 Pump", &terms("pump 台北")));
        assert!(title_matches("EQ-001 台北 Pump", &terms("")));
        assert!(title_matches("EQ-001 台北 Pump", &terms("eq-0")));
        assert!(!title_matches("EQ-001 台北 Pump", &terms("pump 高雄")));
        assert!(!title_matches("", &terms("a")));
    }

    #[test]
    fn record_json_shape() {
        let r = Record {
            id: crate::new_id(),
            record_no: Some("SO202611-0001".into()),
            title: "SO202611-0001".into(),
            data: json!({ "qty": 1 }).as_object().unwrap().clone(),
            relations: BTreeMap::new(),
            formula_errors: BTreeMap::new(),
            attachments: BTreeMap::new(),
            lines: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            created_by: None,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            updated_by: None,
            version: 1,
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["data"], json!({ "qty": 1 }));
        assert_eq!(v["record_no"], json!("SO202611-0001"));
        assert_eq!(v["title"], json!("SO202611-0001"));
        assert_eq!(v["created_at"], json!("1970-01-01T00:00:00Z"));
        assert_eq!(v["relations"], json!({}));
        assert_eq!(v["attachments"], json!({}));
        assert_eq!(serde_json::from_value::<Record>(v).unwrap(), r);

        let target = crate::new_id();
        let mut with_relation = r.clone();
        with_relation.relations.insert(
            "equipment".into(),
            RelationRef {
                id: target,
                title: "EQ-001".into(),
                deleted: true,
            },
        );
        let v = serde_json::to_value(&with_relation).unwrap();
        assert_eq!(
            v["relations"],
            json!({ "equipment": { "id": target, "title": "EQ-001", "deleted": true } })
        );
        assert_eq!(serde_json::from_value::<Record>(v).unwrap(), with_relation);

        let file_id = crate::new_id();
        let mut with_file = r.clone();
        with_file.attachments.insert(
            "photo".into(),
            Attachment {
                id: file_id,
                file_name: "報價單.pdf".into(),
                mime_type: "application/pdf".into(),
                size_bytes: 12,
                sha256: "abc".into(),
                created_at: OffsetDateTime::UNIX_EPOCH,
            },
        );
        let v = serde_json::to_value(&with_file).unwrap();
        assert_eq!(v["attachments"]["photo"]["file_name"], json!("報價單.pdf"));
        assert_eq!(v["attachments"]["photo"]["size_bytes"], json!(12));
        assert_eq!(serde_json::from_value::<Record>(v).unwrap(), with_file);
        // 舊版回應沒有 attachments 時視為空
        let mut v = serde_json::to_value(&r).unwrap();
        v.as_object_mut().unwrap().remove("attachments");
        assert_eq!(serde_json::from_value::<Record>(v).unwrap(), r);

        let req: UpdateRecordRequest = serde_json::from_value(json!({ "version": 2 })).unwrap();
        assert!(req.data.is_empty());
        let req: CreateRecordRequest = serde_json::from_value(json!({})).unwrap();
        assert!(req.data.is_empty());
        // 客戶端帶的單號被忽略
        let req: CreateRecordRequest =
            serde_json::from_value(json!({ "record_no": "X-001", "data": {} })).unwrap();
        assert_eq!(req, CreateRecordRequest::default());
    }

    #[test]
    fn lines_json_shape() {
        let line = Record {
            id: crate::new_id(),
            record_no: None,
            title: "濾網".into(),
            data: json!({ "part": "濾網" }).as_object().unwrap().clone(),
            relations: BTreeMap::new(),
            formula_errors: BTreeMap::new(),
            attachments: BTreeMap::new(),
            lines: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            created_by: None,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            updated_by: None,
            version: 1,
        };
        // 列表與明細本身不輸出 lines
        let v = serde_json::to_value(&line).unwrap();
        assert!(v.get("lines").is_none());

        let master = Record {
            lines: Some(BTreeMap::from([
                ("parts".to_owned(), vec![line.clone()]),
                ("labor".to_owned(), vec![]),
            ])),
            ..line.clone()
        };
        let v = serde_json::to_value(&master).unwrap();
        assert_eq!(v["lines"]["parts"][0]["data"], json!({ "part": "濾網" }));
        assert_eq!(v["lines"]["labor"], json!([]));
        assert_eq!(serde_json::from_value::<Record>(v).unwrap(), master);

        let id = crate::new_id();
        let req: UpdateRecordRequest = serde_json::from_value(json!({
            "version": 3,
            "lines": { "parts": [ { "id": id, "data": { "qty": 2 } }, { "data": {} }, {} ] }
        }))
        .unwrap();
        let parts = &req.lines["parts"];
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].id, Some(id));
        assert_eq!(parts[0].data["qty"], 2);
        assert_eq!(parts[1], LineInput::default());
        assert_eq!(parts[2], LineInput::default());
        let req: UpdateRecordRequest = serde_json::from_value(json!({ "version": 3 })).unwrap();
        assert!(req.lines.is_empty());
    }
}

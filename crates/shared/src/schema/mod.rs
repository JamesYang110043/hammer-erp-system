//! 資料類別與欄位定義（`docs/architecture.md` §7.1–§7.3、§8.5）。
//!
//! - 列舉：[`DataType`]、[`EntityKind`]、[`EntityStorage`]、[`FieldStorage`]，
//!   JSON 中為 snake_case 字串，與資料庫 CHECK 限制的值相同
//! - 欄位設定：[`Validation`]、[`SelectOption`]，對應 `field_definitions.validation` / `options` JSONB
//! - API 回應：[`EntityDefinition`]、[`FieldDefinition`]
//! - API 回應：[`EntityDetail`]（資料類別與欄位定義）
//! - API 請求：[`CreateEntityRequest`]、[`UpdateEntityRequest`]、[`CreateFieldRequest`]、
//!   [`UpdateFieldRequest`]、[`DeactivateFieldRequest`]、[`ReorderFieldsRequest`]、
//!   [`ChangeFieldTypeRequest`]
//!
//! # 端點
//!
//! 讀取需要登入；寫入需要 `schema.manage` 權限。
//!
//! | 端點 | request | 成功回應 |
//! |---|---|---|
//! | `GET /api/v1/entities` | | `200` [`EntityDefinition`]`[]`（有效的資料類別） |
//! | `GET /api/v1/entities/{key}` | | `200` [`EntityDetail`] |
//! | `POST /api/v1/entities` | [`CreateEntityRequest`] | `201` [`EntityDetail`] |
//! | `PATCH /api/v1/entities/{key}` | [`UpdateEntityRequest`] | `200` [`EntityDetail`] |
//! | `POST /api/v1/entities/{key}/fields` | [`CreateFieldRequest`] | `201` [`FieldDefinition`] |
//! | `PATCH /api/v1/entities/{key}/fields/{field_key}` | [`UpdateFieldRequest`] | `200` [`FieldDefinition`] |
//! | `POST /api/v1/entities/{key}/fields/{field_key}/deactivate` | [`DeactivateFieldRequest`] | `200` [`FieldDefinition`] |
//! | `PUT /api/v1/entities/{key}/fields/order` | [`ReorderFieldsRequest`] | `200` [`FieldDefinition`]`[]` |
//! | `POST /api/v1/entities/{key}/fields/{field_key}/change-type` | [`ChangeFieldTypeRequest`] | `200` [`FieldDefinition`] |
//!
//! 錯誤：輸入不合法 → `422 VALIDATION`，`details.field` 為 request 的欄位名稱；
//! `version` 不符 → `409 CONFLICT`；資料類別或欄位不存在 → `404 NOT_FOUND`。
//!
//! 新增、修改、停用欄位時，依「可搜尋」、「不可重複」與啟用狀態建立或移除動態索引（§7.5）。
//! 開啟「不可重複」（或重新啟用不可重複的欄位）時，未刪除的資料已有重複的值 →
//! `422 VALIDATION`，`details` 為
//! `{ "field": "is_unique", "total": 重複的值的個數, "values": [{ "value", "count", "record_ids" }] }`，
//! `values` 依值排序，最多 [`MAX_DUPLICATE_VALUES`] 個；每個值的 `record_ids` 依 id 排序，
//! 最多 [`MAX_DUPLICATE_VALUES`] 筆。
//! - 欄位值的 JSON 存值格式：[`value`]
//! - 代碼命名規則：[`key`]
//!
//! 修改請求採「整筆取代」：送出所有可修改的屬性，`null` 代表清除該屬性。
//! 系統資料類別、系統欄位不可修改的屬性（§7.2）必須維持原值，否則伺服器回傳 `422 VALIDATION`。

pub mod key;
pub mod value;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use specta::Type;
use time::OffsetDateTime;

use crate::Id;

pub use key::{InvalidKey, MAX_KEY_LEN, validate_key};
pub use value::{FieldValue, InvalidValue, parse_value, to_json};

/// 欄位資料型態（§7.3）。對應 `field_definitions.data_type`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum DataType {
    /// 單行文字。
    Text,
    /// 多行文字。
    LongText,
    /// 整數。
    Integer,
    /// 小數（指定小數位數）。
    Decimal,
    /// 金額。M2a 只處理數值與小數位數，幣別在 M3 處理。
    Money,
    /// 日期。
    Date,
    /// 日期時間。
    #[serde(rename = "datetime")]
    DateTime,
    /// 勾選。
    Boolean,
    /// 下拉單選。
    Select,
    /// 下拉多選。
    MultiSelect,
    /// 關聯到其他資料類別（M2b）。
    Relation,
    /// 附件（M2b）。
    File,
    /// 公式，唯讀，由系統計算（M2c）。
    Formula,
}

impl DataType {
    /// 所有型態，順序與 §7.3 相同。
    pub const ALL: &[DataType] = &[
        Self::Text,
        Self::LongText,
        Self::Integer,
        Self::Decimal,
        Self::Money,
        Self::Date,
        Self::DateTime,
        Self::Boolean,
        Self::Select,
        Self::MultiSelect,
        Self::Relation,
        Self::File,
        Self::Formula,
    ];

    /// 資料庫與 JSON 使用的字串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::LongText => "long_text",
            Self::Integer => "integer",
            Self::Decimal => "decimal",
            Self::Money => "money",
            Self::Date => "date",
            Self::DateTime => "datetime",
            Self::Boolean => "boolean",
            Self::Select => "select",
            Self::MultiSelect => "multi_select",
            Self::Relation => "relation",
            Self::File => "file",
            Self::Formula => "formula",
        }
    }

    /// 由資料庫字串轉換；不是已定義的型態時回傳 `None`。
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|t| t.as_str() == s)
    }

    /// 目前是否支援此型態的值與建立此型態的欄位。
    ///
    /// `relation`、`file`、`formula` 在 M2b、M2c 才支援，M2a 的 API 不允許建立。
    pub fn is_value_supported(self) -> bool {
        !matches!(self, Self::Relation | Self::File | Self::Formula)
    }

    /// 是否使用 `decimal_places`（`decimal`、`money`）。
    pub fn uses_decimal_places(self) -> bool {
        matches!(self, Self::Decimal | Self::Money)
    }

    /// 是否使用 `options`（`select`、`multi_select`）。
    pub fn uses_options(self) -> bool {
        matches!(self, Self::Select | Self::MultiSelect)
    }
}

/// 資料類別的種類。對應 `entity_definitions.kind`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    /// 主檔。
    Master,
    /// 單據。
    Document,
    /// 明細子表。
    Detail,
}

impl EntityKind {
    pub const ALL: &[EntityKind] = &[Self::Master, Self::Document, Self::Detail];

    /// 資料庫與 JSON 使用的字串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Master => "master",
            Self::Document => "document",
            Self::Detail => "detail",
        }
    }

    /// 由資料庫字串轉換。
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|k| k.as_str() == s)
    }
}

/// 資料類別的儲存方式。對應 `entity_definitions.storage`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum EntityStorage {
    /// 系統資料表。
    Table,
    /// 通用資料表 `custom_records`。
    CustomRecords,
}

impl EntityStorage {
    pub const ALL: &[EntityStorage] = &[Self::Table, Self::CustomRecords];

    /// 資料庫與 JSON 使用的字串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::CustomRecords => "custom_records",
        }
    }

    /// 由資料庫字串轉換。
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|k| k.as_str() == s)
    }
}

/// 欄位的儲存方式。對應 `field_definitions.storage`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum FieldStorage {
    /// 資料表的實體欄位（系統欄位）。
    Column,
    /// `custom_data` / `data` JSONB。
    Json,
}

impl FieldStorage {
    pub const ALL: &[FieldStorage] = &[Self::Column, Self::Json];

    /// 資料庫與 JSON 使用的字串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Column => "column",
            Self::Json => "json",
        }
    }

    /// 由資料庫字串轉換。
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|k| k.as_str() == s)
    }
}

/// 欄位值的驗證規則。對應 `field_definitions.validation` JSONB。沒有設定的規則省略。
///
/// ```json
/// { "min": "0", "max": "100", "max_length": 50, "pattern": "^\\d{8}$" }
/// ```
///
/// `min`、`max` 與 `decimal` 欄位值一樣以字串存放（D9）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct Validation {
    /// 最小值（含），`integer`、`decimal`、`money` 使用。
    #[serde(
        default,
        with = "crate::decimal::serde_str_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[specta(type = Option<String>)]
    pub min: Option<Decimal>,
    /// 最大值（含），`integer`、`decimal`、`money` 使用。
    #[serde(
        default,
        with = "crate::decimal::serde_str_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[specta(type = Option<String>)]
    pub max: Option<Decimal>,
    /// 文字的字元數上限，`text`、`long_text` 使用。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u32>,
    /// 文字必須符合的正規表示式，`text` 使用。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
}

/// `select`、`multi_select` 的選項。對應 `field_definitions.options` JSONB 陣列的元素。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct SelectOption {
    /// 存入資料的值；同一個欄位內不可重複。
    pub value: String,
    /// 顯示名稱。
    pub label: String,
    /// 標籤顏色（Ant Design 的顏色名稱，例如 `blue`）；沒有設定時為 `null`。
    #[serde(default)]
    pub color: Option<String>,
}

/// 資料類別定義（API 回應）。
///
/// 不包含 `table_name` 等資料庫內部資訊。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct EntityDefinition {
    pub id: Id,
    /// 資料類別代碼（建立後不可修改）。
    pub key: String,
    /// 顯示名稱。
    pub label: String,
    /// 圖示名稱；沒有設定時為 `null`。
    pub icon: Option<String>,
    pub kind: EntityKind,
    /// 系統資料類別不可刪除（§7.1）。
    pub is_system: bool,
    pub storage: EntityStorage,
    /// 明細子表所屬的主表；不是明細子表時為 `null`。
    pub parent_entity_id: Option<Id>,
    /// 單號規則；沒有設定時為 `null`。
    pub numbering_pattern: Option<String>,
    /// 資料標題範本，例如 `{record_no} {name}`；沒有設定時為 `null`（套用預設規則，
    /// 見 [`crate::record`] 的「標題」）。
    pub title_template: Option<String>,
    /// 選單排序。
    pub sort_order: i32,
    pub is_active: bool,
    /// 建立時間（RFC 3339）。
    #[serde(with = "time::serde::rfc3339")]
    #[specta(type = String)]
    pub created_at: OffsetDateTime,
    /// 最後修改時間（RFC 3339）。
    #[serde(with = "time::serde::rfc3339")]
    #[specta(type = String)]
    pub updated_at: OffsetDateTime,
    /// 樂觀鎖定版本號（§9.4）。
    pub version: i32,
}

/// 欄位定義（API 回應）。
///
/// 不包含 `column_name`、`formula_deps` 等資料庫內部資訊。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct FieldDefinition {
    pub id: Id,
    pub entity_id: Id,
    /// 欄位代碼（建立後不可修改），也是 JSONB 的鍵。
    pub field_key: String,
    /// 顯示名稱。
    pub label: String,
    pub data_type: DataType,
    /// 系統欄位（§7.2）。
    pub is_system: bool,
    pub storage: FieldStorage,
    pub is_required: bool,
    pub is_visible: bool,
    pub is_readonly: bool,
    pub is_searchable: bool,
    pub is_unique: bool,
    /// 預設值，格式依 [`value`]；沒有設定時為 `null`。TypeScript 為 `unknown`。
    #[specta(type = Option<specta_typescript::Unknown>)]
    pub default_value: Option<Value>,
    /// 驗證規則；沒有設定時為 `null`。
    pub validation: Option<Validation>,
    /// `select`、`multi_select` 的選項；其他型態為 `null`。
    pub options: Option<Vec<SelectOption>>,
    /// `decimal`、`money` 的小數位數；其他型態為 `null`。
    pub decimal_places: Option<i16>,
    /// `relation` 關聯的資料類別；其他型態為 `null`。
    pub relation_entity_id: Option<Id>,
    /// `formula` 的公式；其他型態為 `null`。
    pub formula: Option<String>,
    /// `formula` 的結果型態；其他型態為 `null`。
    pub formula_result_type: Option<DataType>,
    /// 選用的輸入元件；`null` 代表依型態使用預設元件。
    pub ui_widget: Option<String>,
    /// 列表欄寬（px）；`null` 代表自動。
    pub column_width: Option<i32>,
    /// 說明文字。
    pub help_text: Option<String>,
    pub sort_order: i32,
    /// `false` 代表已停用；自訂欄位只能停用，資料保留（§7.2）。
    pub is_active: bool,
    /// 建立時間（RFC 3339）。
    #[serde(with = "time::serde::rfc3339")]
    #[specta(type = String)]
    pub created_at: OffsetDateTime,
    /// 最後修改時間（RFC 3339）。
    #[serde(with = "time::serde::rfc3339")]
    #[specta(type = String)]
    pub updated_at: OffsetDateTime,
    /// 樂觀鎖定版本號（§9.4）。
    pub version: i32,
}

/// 建立自訂資料類別的輸入。儲存方式固定為 `custom_records`。
///
/// M2a 只接受 `kind = master`；明細子表在 M2b 支援。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CreateEntityRequest {
    /// 資料類別代碼，須符合 [`validate_key`]；建立後不可修改。
    pub key: String,
    /// 顯示名稱（前後空白會去除）。
    pub label: String,
    #[serde(default)]
    pub icon: Option<String>,
    pub kind: EntityKind,
    /// 單號規則，例如 `SO{yyyyMM}-{0000}`（格式見 architecture.md §8.4）；空白或 `null` 代表不使用單號。
    /// 格式不合法時回傳 `422 VALIDATION`，`details.field` 為 `numbering_pattern`。
    #[serde(default)]
    pub numbering_pattern: Option<String>,
    /// 資料標題範本，例如 `{record_no} {name}`：`{}` 內為 `record_no` 或本資料類別的欄位代碼，
    /// 其他文字原樣保留。新的資料類別還沒有欄位，只能引用 `{record_no}`。
    /// 語法不合法或引用不存在的欄位時回傳 `422 VALIDATION`，`details.field` 為 `title_template`。
    #[serde(default)]
    pub title_template: Option<String>,
    /// 選單排序（預設 0）。
    #[serde(default)]
    pub sort_order: i32,
}

/// 修改資料類別的輸入（整筆取代可修改的屬性）。`key`、`kind`、`storage` 不可修改。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct UpdateEntityRequest {
    /// 讀取時取得的版本號；不符時回傳 `409 CONFLICT`。
    pub version: i32,
    /// 選填。資料類別代碼不可修改；帶了且與目前的值不同時回傳 `422 VALIDATION`。
    #[serde(default)]
    pub key: Option<String>,
    pub label: String,
    /// `null` 代表清除。
    pub icon: Option<String>,
    /// 單號規則（格式同 [`CreateEntityRequest::numbering_pattern`]）；`null` 或空白代表清除。
    /// 修改後只影響之後新增的資料，既有的單號不變。
    #[serde(default)]
    pub numbering_pattern: Option<String>,
    /// 資料標題範本（格式同 [`CreateEntityRequest::title_template`]）；只能引用此資料類別已定義的
    /// 欄位（含已停用）。`null` 或空白代表清除。
    pub title_template: Option<String>,
    pub sort_order: i32,
    /// 停用後不出現在選單；系統資料類別不可停用。
    pub is_active: bool,
}

/// 建立欄位的輸入。儲存方式固定為 `json`。
///
/// M2a 不允許 `relation`、`file`、`formula`（見 [`DataType::is_value_supported`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CreateFieldRequest {
    /// 欄位代碼，須符合 [`validate_key`]；建立後不可修改。
    pub field_key: String,
    /// 顯示名稱（前後空白會去除）。
    pub label: String,
    pub data_type: DataType,
    #[serde(default)]
    pub is_required: bool,
    /// 預設 `true`。
    #[serde(default = "default_true")]
    pub is_visible: bool,
    #[serde(default)]
    pub is_readonly: bool,
    #[serde(default)]
    pub is_searchable: bool,
    #[serde(default)]
    pub is_unique: bool,
    /// 預設值，格式依 [`value`]。TypeScript 為 `unknown`。
    #[serde(default)]
    #[specta(type = Option<specta_typescript::Unknown>)]
    pub default_value: Option<Value>,
    #[serde(default)]
    pub validation: Option<Validation>,
    /// `select`、`multi_select` 必填。
    #[serde(default)]
    pub options: Option<Vec<SelectOption>>,
    /// `decimal`、`money` 必填。
    #[serde(default)]
    pub decimal_places: Option<i16>,
    #[serde(default)]
    pub ui_widget: Option<String>,
    #[serde(default)]
    pub column_width: Option<i32>,
    #[serde(default)]
    pub help_text: Option<String>,
    /// 排序（預設 0）。
    #[serde(default)]
    pub sort_order: i32,
}

/// 修改欄位的輸入（整筆取代可修改的屬性）。`field_key`、`storage` 不可修改。
///
/// 變更 `data_type` 使用 [`ChangeFieldTypeRequest`]。
/// 系統欄位只能修改顯示名稱、顯示或隱藏、排序、欄寬、說明文字、必填與預設值（§7.2），
/// 其他屬性必須維持原值。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct UpdateFieldRequest {
    /// 讀取時取得的版本號；不符時回傳 `409 CONFLICT`。
    pub version: i32,
    /// 選填。欄位代碼不可修改；帶了且與目前的值不同時回傳 `422 VALIDATION`。
    #[serde(default)]
    pub field_key: Option<String>,
    pub label: String,
    /// 目前必須與原本的型態相同（型態變更另有 API）；不同時回傳 `422 VALIDATION`。
    pub data_type: DataType,
    pub is_required: bool,
    pub is_visible: bool,
    pub is_readonly: bool,
    pub is_searchable: bool,
    pub is_unique: bool,
    /// `null` 代表清除。TypeScript 為 `unknown`。
    #[specta(type = Option<specta_typescript::Unknown>)]
    pub default_value: Option<Value>,
    /// `null` 代表清除。
    pub validation: Option<Validation>,
    pub options: Option<Vec<SelectOption>>,
    pub decimal_places: Option<i16>,
    pub ui_widget: Option<String>,
    pub column_width: Option<i32>,
    pub help_text: Option<String>,
    pub sort_order: i32,
    /// 停用欄位（§7.2），資料保留。系統欄位不可停用。
    pub is_active: bool,
}

/// 停用欄位的輸入（§7.2）。資料保留；系統欄位不可停用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct DeactivateFieldRequest {
    /// 讀取時取得的版本號；不符時回傳 `409 CONFLICT`。
    pub version: i32,
}

/// 變更自訂欄位資料型態的輸入（§7.4）。系統欄位不可變更型態（§7.2）。
///
/// 依新型態整筆取代 `decimal_places`、`options`、`validation`（不使用的設定省略或 `null`），
/// 其他屬性不變；`default_value` 轉換成新型態。
///
/// 伺服器在同一個 transaction 內把該資料類別的**所有**資料（含已軟刪除）轉換成新型態並更新欄位定義。
/// 轉換規則：
///
/// | 從 → 到 | 規則 |
/// |---|---|
/// | 任意型態 → `text` / `long_text` | 允許（轉成文字） |
/// | `text` ↔ `long_text` | 允許 |
/// | `integer` → `decimal` | 允許 |
/// | `select` → `multi_select` | 允許（變成只有一個選項的陣列） |
/// | 其他 | 逐筆轉換，只要有一筆失敗就拒絕 |
///
/// 轉換後的值仍須符合新的設定（選項、小數位數、驗證規則），「不可重複」的欄位轉換後也不可重複。
/// `relation`、`file`、`formula` 不能轉換。
///
/// 錯誤：
/// - 有資料無法轉換 → `422 VALIDATION`，`details` 為
///   `{ "field": "data_type", "total": 失敗總筆數, "records": [{ "id", "value", "deleted", "message" }] }`，
///   `records` 最多 [`MAX_CONVERSION_FAILURES`] 筆，`value` 為原本的值，`deleted` 表示該筆資料已軟刪除
/// - 設定不合法、系統欄位、型態相同或不支援 → `422 VALIDATION`，`details.field` 為 request 的欄位名稱
/// - `version` 不符 → `409 CONFLICT`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ChangeFieldTypeRequest {
    /// 讀取時取得的欄位版本號；不符時回傳 `409 CONFLICT`。
    pub version: i32,
    /// 新的資料型態，必須與目前不同。
    pub data_type: DataType,
    /// `decimal`、`money` 必填。
    #[serde(default)]
    pub decimal_places: Option<i16>,
    /// `select`、`multi_select` 必填。
    #[serde(default)]
    pub options: Option<Vec<SelectOption>>,
    /// 新型態的驗證規則；省略或 `null` 代表不設定。
    #[serde(default)]
    pub validation: Option<Validation>,
}

/// 型態變更失敗時，`details.records` 最多列出的筆數。
pub const MAX_CONVERSION_FAILURES: usize = 100;

/// 無法開啟「不可重複」時，`details.values` 最多列出的值的個數，以及每個值最多列出的資料 id 筆數。
pub const MAX_DUPLICATE_VALUES: usize = 100;

/// 資料類別與其所有欄位定義（含已停用），欄位依 `sort_order` 排序。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct EntityDetail {
    pub entity: EntityDefinition,
    pub fields: Vec<FieldDefinition>,
}

/// 調整欄位排序的輸入。
///
/// `fields` 依新的順序列出該資料類別的**所有**欄位（含已停用），每個欄位恰好一次；
/// 第 n 個欄位的 `sort_order` 設為 n（從 0 開始）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ReorderFieldsRequest {
    pub fields: Vec<FieldOrderItem>,
}

/// [`ReorderFieldsRequest`] 的一個欄位。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct FieldOrderItem {
    pub field_key: String,
    /// 讀取時取得的欄位版本號；任一欄位不符時整批不寫入，回傳 `409 CONFLICT`。
    pub version: i32,
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashSet;
    use time::macros::datetime;

    /// 與 `0003_customization_tables.sql` 的 CHECK 限制相同。
    #[test]
    fn data_type_strings_match_database() {
        let strings: Vec<_> = DataType::ALL.iter().map(|t| t.as_str()).collect();
        assert_eq!(
            strings,
            [
                "text",
                "long_text",
                "integer",
                "decimal",
                "money",
                "date",
                "datetime",
                "boolean",
                "select",
                "multi_select",
                "relation",
                "file",
                "formula",
            ]
        );
        assert_eq!(strings.iter().collect::<HashSet<_>>().len(), strings.len());
    }

    #[test]
    fn enum_serde_matches_as_str_and_parse() {
        for t in DataType::ALL {
            assert_eq!(serde_json::to_value(t).unwrap(), json!(t.as_str()));
            assert_eq!(DataType::parse(t.as_str()), Some(*t));
        }
        for k in EntityKind::ALL {
            assert_eq!(serde_json::to_value(k).unwrap(), json!(k.as_str()));
            assert_eq!(EntityKind::parse(k.as_str()), Some(*k));
        }
        for s in EntityStorage::ALL {
            assert_eq!(serde_json::to_value(s).unwrap(), json!(s.as_str()));
            assert_eq!(EntityStorage::parse(s.as_str()), Some(*s));
        }
        for s in FieldStorage::ALL {
            assert_eq!(serde_json::to_value(s).unwrap(), json!(s.as_str()));
            assert_eq!(FieldStorage::parse(s.as_str()), Some(*s));
        }
        assert_eq!(DataType::parse("Text"), None);
        assert_eq!(DataType::parse("date_time"), None);
        assert!(serde_json::from_value::<DataType>(json!("float")).is_err());
    }

    #[test]
    fn unsupported_types_in_m2a() {
        let unsupported: Vec<_> = DataType::ALL
            .iter()
            .filter(|t| !t.is_value_supported())
            .collect();
        assert_eq!(
            unsupported,
            [&DataType::Relation, &DataType::File, &DataType::Formula]
        );
    }

    #[test]
    fn validation_json_shape() {
        let v = Validation {
            min: Some(Decimal::new(0, 0)),
            max: Some(Decimal::new(10050, 2)),
            max_length: None,
            pattern: Some(r"^\d{8}$".into()),
        };
        let value = serde_json::to_value(&v).unwrap();
        assert_eq!(
            value,
            json!({ "min": "0", "max": "100.50", "pattern": "^\\d{8}$" })
        );
        assert_eq!(serde_json::from_value::<Validation>(value).unwrap(), v);
        assert_eq!(
            serde_json::from_value::<Validation>(json!({})).unwrap(),
            Validation::default()
        );
        assert_eq!(
            serde_json::to_value(Validation::default()).unwrap(),
            json!({})
        );
        // 數值必須是字串（D9）
        assert!(serde_json::from_value::<Validation>(json!({ "min": 0 })).is_err());
    }

    #[test]
    fn select_option_color_is_optional() {
        let o: SelectOption =
            serde_json::from_value(json!({ "value": "a", "label": "A" })).unwrap();
        assert_eq!(o.color, None);
        let o: SelectOption =
            serde_json::from_value(json!({ "value": "a", "label": "A", "color": "blue" })).unwrap();
        assert_eq!(o.color.as_deref(), Some("blue"));
    }

    #[test]
    fn field_definition_json_round_trip() {
        let field = FieldDefinition {
            id: crate::new_id(),
            entity_id: crate::new_id(),
            field_key: "status".into(),
            label: "狀態".into(),
            data_type: DataType::Select,
            is_system: false,
            storage: FieldStorage::Json,
            is_required: true,
            is_visible: true,
            is_readonly: false,
            is_searchable: true,
            is_unique: false,
            default_value: Some(json!("open")),
            validation: None,
            options: Some(vec![SelectOption {
                value: "open".into(),
                label: "處理中".into(),
                color: Some("blue".into()),
            }]),
            decimal_places: None,
            relation_entity_id: None,
            formula: None,
            formula_result_type: None,
            ui_widget: None,
            column_width: Some(120),
            help_text: None,
            sort_order: 1,
            is_active: true,
            created_at: datetime!(2026-11-13 01:00 UTC),
            updated_at: datetime!(2026-11-13 09:00 +8),
            version: 1,
        };
        let value = serde_json::to_value(&field).unwrap();
        assert_eq!(value["data_type"], json!("select"));
        assert_eq!(value["storage"], json!("json"));
        assert_eq!(value["created_at"], json!("2026-11-13T01:00:00Z"));
        assert_eq!(
            serde_json::from_value::<FieldDefinition>(value).unwrap(),
            field
        );
    }

    #[test]
    fn create_field_request_defaults() {
        let req: CreateFieldRequest = serde_json::from_value(json!({
            "field_key": "amount",
            "label": "金額",
            "data_type": "money",
            "decimal_places": 2,
        }))
        .unwrap();
        assert_eq!(req.data_type, DataType::Money);
        assert!(req.is_visible);
        assert!(!req.is_required && !req.is_readonly && !req.is_searchable && !req.is_unique);
        assert_eq!(req.decimal_places, Some(2));
        assert_eq!(req.sort_order, 0);
        assert_eq!(req.default_value, None);
    }

    #[test]
    fn create_entity_request_defaults() {
        let req: CreateEntityRequest = serde_json::from_value(json!({
            "key": "repair_order",
            "label": "維修單",
            "kind": "master",
        }))
        .unwrap();
        assert_eq!(req.kind, EntityKind::Master);
        assert_eq!(req.icon, None);
        assert_eq!(req.sort_order, 0);
    }
}

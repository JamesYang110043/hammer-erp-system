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
//!
//! 讀取需要 `record.read`，寫入需要 `record.write`。`data` 的格式見 [`crate::schema::value`]。
//!
//! 錯誤：
//!
//! - 資料類別不存在、已停用或不是自訂資料類別；資料不存在或已刪除 → `404 NOT_FOUND`
//! - `data` 不符合欄位定義（含違反「不可重複」）→ `422 VALIDATION`，
//!   `details.fields` 為 `[{ "field_key", "message" }]`，列出所有出錯的欄位
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
//! - 多個篩選條件須全部符合（AND）；同一個欄位的多個 `any` 合併為一個條件（符合任一個即可）
//! - 值為空字串的條件視為未指定
//! - 依欄位型態比較與排序（數值不會按字串排序）；沒有值的資料排在最後；
//!   次要排序為 `id`（方向與主要排序相同）
//! - 其他參數名稱忽略
//!
//! [`RecordListQuery::from_pairs`] 解析語法；欄位是否存在、運算子是否適用於該型態、
//! 值的格式由伺服器依欄位定義檢查。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use specta::Type;
use time::OffsetDateTime;

use crate::Id;
use crate::pagination::field as page_field;

/// 一筆自訂資料類別的資料。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct Record {
    pub id: Id,
    /// 所有欄位的值，鍵為 `field_key`，格式依 [`crate::schema::value`]。沒有值的欄位不會出現。
    #[specta(type = std::collections::HashMap<String, specta_typescript::Unknown>)]
    pub data: Map<String, Value>,
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

/// 新增資料的輸入。沒有帶到的欄位套用預設值。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
pub struct CreateRecordRequest {
    #[serde(default)]
    #[specta(type = std::collections::HashMap<String, specta_typescript::Unknown>)]
    pub data: Map<String, Value>,
}

/// 修改資料的輸入。只合併有帶到的欄位，值為 `null` 代表清空。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct UpdateRecordRequest {
    /// 讀取時取得的版本號；不符時回傳 `409 CONFLICT`。
    pub version: i32,
    #[serde(default)]
    #[specta(type = std::collections::HashMap<String, specta_typescript::Unknown>)]
    pub data: Map<String, Value>,
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
    fn record_json_shape() {
        let r = Record {
            id: crate::new_id(),
            data: json!({ "qty": 1 }).as_object().unwrap().clone(),
            created_at: OffsetDateTime::UNIX_EPOCH,
            created_by: None,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            updated_by: None,
            version: 1,
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["data"], json!({ "qty": 1 }));
        assert_eq!(v["created_at"], json!("1970-01-01T00:00:00Z"));
        assert_eq!(serde_json::from_value::<Record>(v).unwrap(), r);

        let req: UpdateRecordRequest = serde_json::from_value(json!({ "version": 2 })).unwrap();
        assert!(req.data.is_empty());
        let req: CreateRecordRequest = serde_json::from_value(json!({})).unwrap();
        assert!(req.data.is_empty());
    }
}

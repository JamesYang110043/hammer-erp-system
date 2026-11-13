//! 列表版面（`docs/architecture.md` §8.5 `view_layouts`、§9.4、§10）。
//!
//! 使用者可以儲存自己的列表版面（個人版面）；管理者可以設定所有人共用的全域版面
//! （`user_id` 為 `null`）。
//!
//! | 端點 | request | 成功回應 |
//! |---|---|---|
//! | `GET /api/v1/entities/{key}/layouts` | | `200` [`ViewLayout`]`[]`（全域版面，以及目前使用者自己的版面） |
//! | `GET /api/v1/entities/{key}/layouts/default` | | `200` [`DefaultViewLayout`] |
//! | `POST /api/v1/entities/{key}/layouts` | [`CreateViewLayoutRequest`] | `201` [`ViewLayout`] |
//! | `PATCH /api/v1/entities/{key}/layouts/{id}` | [`UpdateViewLayoutRequest`] | `200` [`ViewLayout`] |
//! | `DELETE /api/v1/entities/{key}/layouts/{id}?version=N` | [`DeleteViewLayoutQuery`] | `204 No Content` |
//!
//! 權限：
//!
//! - 讀取、新增或修改自己的個人版面：`record.read`
//! - 新增、修改、刪除全域版面：`schema.manage`
//! - 個人版面只有本人可以修改；別人的個人版面視為不存在（`404`），管理者也一樣
//!
//! 列表的排序：全域版面在前，再依名稱、建立順序。
//!
//! # 預設版面
//!
//! 每個使用者在同一個資料類別最多一個預設版面，全域預設也只有一個。新增或修改時把
//! `is_default` 設為 `true`，同一範圍（同一個使用者，或全域）原本的預設會被取消
//! （該版面的 `version` 遞增）。
//!
//! `GET …/layouts/default` 的取得順序：使用者自己的預設版面 → 全域預設版面 →
//! 依欄位定義自動產生（[`LayoutSource`]）。
//!
//! # `config`
//!
//! [`LayoutConfig`]：顯示的欄位與順序、欄寬、排序、篩選條件。排序與篩選的格式與通用資料 API
//! 相同（[`crate::record`]）。
//!
//! - 寫入時檢查：欄位必須是該資料類別有效（未停用）的欄位、不可重複；欄寬大於 0；
//!   排序與篩選的規則與通用資料 API 相同
//! - 讀取時：參照到已停用或不存在的欄位（或欄位型態變更後已不適用的排序、篩選條件）
//!   直接略過，不回傳錯誤
//!
//! 錯誤：
//!
//! - 資料類別不存在或已停用；版面不存在、已刪除或是別人的個人版面 → `404 NOT_FOUND`
//! - 沒有權限 → `403 FORBIDDEN`
//! - 輸入不合法 → `422 VALIDATION`，`details.field` 為欄位路徑，例如 `name`、
//!   `config.columns[0].field_key`、`config.sort`、`config.filters[1]`
//! - `version` 不符，或同時有其他人設定了預設版面 → `409 CONFLICT`（§9.4）

use serde::{Deserialize, Serialize};
use specta::Type;
use time::OffsetDateTime;

use crate::Id;
use crate::record::{RecordFilter, RecordSort};

/// 版面名稱的長度上限（字元）。
pub const MAX_LAYOUT_NAME_CHARS: usize = 100;

/// 列表的一個顯示欄位。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct LayoutColumn {
    pub field_key: String,
    /// 欄寬（px）；`null` 代表使用欄位定義的 `column_width`。
    #[serde(default)]
    pub width: Option<i32>,
}

/// 版面設定（`view_layouts.config`）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct LayoutConfig {
    /// 顯示的欄位，依顯示順序。
    #[serde(default)]
    pub columns: Vec<LayoutColumn>,
    /// 排序；`null` 代表依建立順序由新到舊。
    #[serde(default)]
    pub sort: Option<RecordSort>,
    /// 篩選條件，須全部符合（AND）；同一個欄位的多個 `any` 符合任一個即可。
    #[serde(default)]
    pub filters: Vec<RecordFilter>,
}

/// 一個列表版面。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct ViewLayout {
    pub id: Id,
    /// 擁有者；`null` 代表全域版面。
    pub user_id: Option<Id>,
    pub name: String,
    /// 已略過參照到停用或不存在欄位的部分。
    pub config: LayoutConfig,
    pub is_default: bool,
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

impl ViewLayout {
    /// 是否為全域版面。
    pub fn is_global(&self) -> bool {
        self.user_id.is_none()
    }
}

/// 新增版面的輸入。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct CreateViewLayoutRequest {
    pub name: String,
    /// `true`：全域版面（需要 `schema.manage`）；預設為個人版面。
    #[serde(default)]
    pub is_global: bool,
    #[serde(default)]
    pub config: LayoutConfig,
    /// `true`：設為預設，並取消同一範圍原本的預設。
    #[serde(default)]
    pub is_default: bool,
}

/// 修改版面的輸入。只修改有帶到的屬性；個人或全域建立後不可變更。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct UpdateViewLayoutRequest {
    /// 讀取時取得的版本號；不符時回傳 `409 CONFLICT`。
    pub version: i32,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub config: Option<LayoutConfig>,
    /// `true`：設為預設，並取消同一範圍原本的預設；`false`：取消預設。
    #[serde(default)]
    pub is_default: Option<bool>,
}

/// `DELETE /api/v1/entities/{key}/layouts/{id}` 的 query string。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct DeleteViewLayoutQuery {
    /// 讀取時取得的版本號；不符時回傳 `409 CONFLICT`。
    pub version: i32,
}

/// 預設版面的來源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum LayoutSource {
    /// 使用者自己的預設版面。
    Personal,
    /// 全域預設版面。
    Global,
    /// 沒有預設版面，依欄位定義自動產生：有效且顯示（`is_visible`）的欄位，
    /// 依欄位排序，欄寬為欄位定義的 `column_width`；不排序、不篩選。
    Generated,
}

/// `GET /api/v1/entities/{key}/layouts/default` 的回應。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct DefaultViewLayout {
    pub source: LayoutSource,
    /// 使用的版面；`source = generated` 時為 `null`。
    pub layout: Option<ViewLayout>,
    /// 要套用的設定；有版面時與 `layout.config` 相同。
    pub config: LayoutConfig,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::FilterOp;
    use serde_json::json;

    #[test]
    fn config_defaults_to_empty() {
        let config: LayoutConfig = serde_json::from_value(json!({})).unwrap();
        assert_eq!(config, LayoutConfig::default());
        let column: LayoutColumn = serde_json::from_value(json!({ "field_key": "a" })).unwrap();
        assert_eq!(column.width, None);
    }

    #[test]
    fn config_json_shape() {
        let config = LayoutConfig {
            columns: vec![LayoutColumn {
                field_key: "qty".into(),
                width: Some(120),
            }],
            sort: Some(RecordSort {
                field_key: "qty".into(),
                descending: true,
            }),
            filters: vec![RecordFilter {
                field_key: "qty".into(),
                op: FilterOp::Gte,
                value: "10".into(),
            }],
        };
        let v = serde_json::to_value(&config).unwrap();
        assert_eq!(
            v,
            json!({
                "columns": [{ "field_key": "qty", "width": 120 }],
                "sort": { "field_key": "qty", "descending": true },
                "filters": [{ "field_key": "qty", "op": "gte", "value": "10" }],
            })
        );
        assert_eq!(serde_json::from_value::<LayoutConfig>(v).unwrap(), config);
    }

    #[test]
    fn requests_default_optional_fields() {
        let req: CreateViewLayoutRequest = serde_json::from_value(json!({ "name": "x" })).unwrap();
        assert!(!req.is_global && !req.is_default);
        assert_eq!(req.config, LayoutConfig::default());

        let req: UpdateViewLayoutRequest = serde_json::from_value(json!({ "version": 1 })).unwrap();
        assert_eq!((req.name, req.config, req.is_default), (None, None, None));
    }

    #[test]
    fn source_strings() {
        assert_eq!(
            serde_json::to_value(LayoutSource::Generated).unwrap(),
            json!("generated")
        );
    }
}

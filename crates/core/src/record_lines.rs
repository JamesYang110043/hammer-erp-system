//! 明細子表的明細與主表一起存檔時的檢查（docs/architecture.md §8.5 `custom_records`、§10；
//! 格式見 [`hammer_shared::record`]「明細子表」）。
//!
//! 這裡只有不存取資料庫的邏輯，讀寫在 [`crate::records`]：
//!
//! - [`unknown_details`]：request 中的明細子表代碼必須是主表有效的明細子表
//! - [`match_lines`]：比對一個明細子表的輸入與既有明細（整組取代：新增、更新、刪除）
//! - [`duplicate_line_values`]：同一次存檔的明細中，「不可重複」欄位的值重複
//! - [`duplicate_attachments`]：同一個附件用在主表與明細的多個地方
//! - [`audit_key`]、[`audit_value`]：明細在主表操作紀錄中的鍵與值
//! - [`validation_error`]：合併主表與明細的驗證錯誤
//!
//! 錯誤以 [`LineError`] 表示，`index` 為該列在 request 陣列中的位置（從 0 開始）。

use std::collections::{BTreeMap, HashMap, HashSet};

use hammer_shared::Id;
use hammer_shared::record::LineInput;
use hammer_shared::schema::{EntityDefinition, FieldDefinition, FieldStorage};
use serde_json::{Map, Value, json};

use crate::CoreError;
use crate::attachments::{FileValue, MSG_ATTACHMENT_IN_USE};
use crate::record_data::FieldError;

/// request 中的明細子表代碼不是主表有效的明細子表。
pub const MSG_UNKNOWN_DETAIL: &str = "沒有此明細子表";
/// 新增資料時明細帶了 `id`。
pub const MSG_ID_ON_CREATE: &str = "新增資料時明細不可指定 id";
/// 修改時明細的 `id` 不是這筆資料在此明細子表的未刪除明細。
pub const MSG_LINE_NOT_FOUND: &str = "找不到此明細，可能已被刪除，請重新載入";
/// 同一個 `id` 出現兩次。
pub const MSG_DUPLICATE_LINE: &str = "同一列明細不可出現兩次";
/// 同一次存檔的明細中，「不可重複」欄位的值重複。
pub const MSG_DUPLICATE_VALUE: &str = "此值與其他明細重複";

/// 一個明細的錯誤。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineError {
    /// 明細子表的代碼（request 中 `lines` 的鍵）。空字串表示不屬於明細子表（例如建議批號 API 的
    /// `lines`），API 回應為 `null`。
    pub detail_key: String,
    /// 該列在 request 陣列中的位置（從 0 開始）；不屬於某一列（例如明細子表不存在）時為 `None`。
    pub index: Option<usize>,
    /// 出錯的欄位代碼；不屬於某個欄位（例如 `id` 不存在）時為 `None`。
    pub field_key: Option<String>,
    /// 顯示給使用者的說明（繁體中文）。
    pub message: String,
}

impl LineError {
    /// 整個明細子表的錯誤。
    pub fn detail(detail_key: &str, message: impl Into<String>) -> Self {
        Self {
            detail_key: detail_key.to_owned(),
            index: None,
            field_key: None,
            message: message.into(),
        }
    }

    /// 一列明細的錯誤（不屬於某個欄位）。
    pub fn row(detail_key: &str, index: usize, message: impl Into<String>) -> Self {
        Self {
            index: Some(index),
            ..Self::detail(detail_key, message)
        }
    }

    /// 一列明細的某個欄位的錯誤。
    pub fn field(
        detail_key: &str,
        index: usize,
        field_key: &str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            field_key: Some(field_key.to_owned()),
            ..Self::row(detail_key, index, message)
        }
    }

    /// 一列明細的欄位錯誤（[`crate::record_data`]、關聯、附件的檢查結果）。
    pub fn from_fields(
        detail_key: &str,
        index: usize,
        errors: Vec<FieldError>,
    ) -> impl Iterator<Item = Self> {
        let detail_key = detail_key.to_owned();
        errors
            .into_iter()
            .map(move |e| Self::field(&detail_key, index, &e.field_key, e.message))
    }
}

/// `lines` 中不是 `details`（主表有效的明細子表）的代碼，依代碼排序。
pub fn unknown_details(
    lines: &BTreeMap<String, Vec<LineInput>>,
    details: &[&EntityDefinition],
) -> Vec<LineError> {
    lines
        .keys()
        .filter(|key| !details.iter().any(|d| d.key == **key))
        .map(|key| LineError::detail(key, MSG_UNKNOWN_DETAIL))
        .collect()
}

/// 一列輸入對應的明細。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineTarget {
    /// 新增。
    New,
    /// 更新既有的明細；值為在 `existing_ids` 中的位置。
    Existing(usize),
}

/// [`match_lines`] 的結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineMatch {
    /// 每一列輸入對應的明細，與輸入的順序相同。
    pub targets: Vec<LineTarget>,
    /// 沒有列出、要刪除的既有明細，為在 `existing_ids` 中的位置（依 `existing_ids` 的順序）。
    pub removed: Vec<usize>,
}

/// 比對一個明細子表的輸入與既有明細（`existing_ids`：這筆主表在此明細子表的未刪除明細）。
///
/// - 有 `id` 的更新：`id` 必須在 `existing_ids` 中（[`MSG_LINE_NOT_FOUND`]），且不可出現兩次
///   （[`MSG_DUPLICATE_LINE`]）；新增資料（`creating`）時不可帶 `id`（[`MSG_ID_ON_CREATE`]）
/// - 沒有 `id` 的新增
/// - 沒有列出的既有明細刪除
///
/// 有任何錯誤時回傳所有出錯的列（依輸入的順序）。
pub fn match_lines(
    detail_key: &str,
    inputs: &[LineInput],
    existing_ids: &[Id],
    creating: bool,
) -> Result<LineMatch, Vec<LineError>> {
    let position: HashMap<Id, usize> = existing_ids
        .iter()
        .enumerate()
        .map(|(i, id)| (*id, i))
        .collect();
    let mut used = HashSet::new();
    let mut targets = Vec::with_capacity(inputs.len());
    let mut errors = Vec::new();
    for (index, input) in inputs.iter().enumerate() {
        let Some(id) = input.id else {
            targets.push(LineTarget::New);
            continue;
        };
        let message = if creating {
            MSG_ID_ON_CREATE
        } else if let Some(&i) = position.get(&id) {
            if used.insert(i) {
                targets.push(LineTarget::Existing(i));
                continue;
            }
            MSG_DUPLICATE_LINE
        } else {
            MSG_LINE_NOT_FOUND
        };
        errors.push(LineError::row(detail_key, index, message));
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let removed = (0..existing_ids.len())
        .filter(|i| !used.contains(i))
        .collect();
    Ok(LineMatch { targets, removed })
}

/// 同一個明細子表、同一次存檔的明細中，「不可重複」（`is_unique`）欄位的值與前面的列相同的列
/// （依列、欄位的順序）。`data` 與輸入的順序相同；驗證失敗的列為 `None`，不參與比較。
///
/// 與其他主表的明細重複由資料庫的唯一索引檢查（§7.5）。
pub fn duplicate_line_values(
    detail_key: &str,
    fields: &[FieldDefinition],
    data: &[Option<&Map<String, Value>>],
) -> Vec<LineError> {
    let unique: Vec<&FieldDefinition> = fields
        .iter()
        .filter(|f| f.is_unique && f.is_active && f.storage == FieldStorage::Json)
        .collect();
    let mut seen: HashMap<&str, HashSet<&Value>> = HashMap::new();
    let mut errors = Vec::new();
    for (index, line) in data.iter().enumerate() {
        let Some(line) = line else {
            continue;
        };
        for field in &unique {
            let key = field.field_key.as_str();
            let Some(value) = line.get(key) else {
                continue;
            };
            if !seen.entry(key).or_default().insert(value) {
                errors.push(LineError::field(
                    detail_key,
                    index,
                    key,
                    MSG_DUPLICATE_VALUE,
                ));
            }
        }
    }
    errors
}

/// 一列明細要綁定的附件（[`duplicate_attachments`] 的輸入）。
#[derive(Debug, Clone, Copy)]
pub struct LineFiles<'a> {
    pub detail_key: &'a str,
    pub index: usize,
    pub bind: &'a [FileValue],
}

/// 同一次存檔中，同一個附件只能用在一個地方：與主表（`master`）或前面的列重複的明細欄位
/// （[`MSG_ATTACHMENT_IN_USE`]）。主表內、同一列內的重複已在 [`crate::attachments`] 檢查。
pub fn duplicate_attachments(master: &[FileValue], lines: &[LineFiles<'_>]) -> Vec<LineError> {
    let mut used: HashSet<Id> = master.iter().map(|v| v.id).collect();
    let mut errors = Vec::new();
    for line in lines {
        let mut in_line = HashSet::new();
        for v in line.bind {
            if used.contains(&v.id) {
                errors.push(LineError::field(
                    line.detail_key,
                    line.index,
                    &v.field_key,
                    MSG_ATTACHMENT_IN_USE,
                ));
            }
            in_line.insert(v.id);
        }
        used.extend(in_line);
    }
    errors
}

/// 明細子表在主表操作紀錄中的鍵：`lines.<明細子表代碼>`。欄位代碼不能有 `.`，不會與欄位衝突。
pub fn audit_key(detail_key: &str) -> String {
    format!("lines.{detail_key}")
}

/// 明細子表在主表操作紀錄中的值：依順序的 `[{ "id", "data" }]`。
pub fn audit_value<'a>(lines: impl IntoIterator<Item = (Id, &'a Value)>) -> Value {
    Value::Array(
        lines
            .into_iter()
            .map(|(id, data)| json!({ "id": id, "data": data }))
            .collect(),
    )
}

/// 合併主表（`master`：[`crate::record_data::normalize_record_data`] 的結果）與明細的驗證錯誤：
///
/// - 沒有明細錯誤 → 主表的結果（錯誤為 [`CoreError::InvalidFields`]）
/// - 有明細錯誤 → [`CoreError::InvalidLines`]，`fields` 為主表的欄位錯誤（可能為空）
pub fn validation_error(
    master: Result<Map<String, Value>, Vec<FieldError>>,
    lines: Vec<LineError>,
) -> Result<Map<String, Value>, CoreError> {
    if lines.is_empty() {
        return master.map_err(CoreError::InvalidFields);
    }
    Err(CoreError::InvalidLines {
        fields: master.err().unwrap_or_default(),
        lines,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hammer_shared::new_id;
    use hammer_shared::schema::{DataType, EntityKind, EntityStorage};
    use time::OffsetDateTime;

    fn input(id: Option<Id>) -> LineInput {
        LineInput {
            id,
            data: Map::new(),
        }
    }

    fn entity(key: &str) -> EntityDefinition {
        EntityDefinition {
            id: new_id(),
            key: key.into(),
            label: key.into(),
            icon: None,
            kind: EntityKind::Detail,
            is_system: false,
            storage: EntityStorage::CustomRecords,
            parent_entity_id: Some(new_id()),
            numbering_pattern: None,
            title_template: None,
            sort_order: 0,
            is_active: true,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            version: 1,
        }
    }

    fn field(key: &str, is_unique: bool) -> FieldDefinition {
        FieldDefinition {
            id: new_id(),
            entity_id: Id::nil(),
            field_key: key.to_owned(),
            label: key.to_owned(),
            data_type: DataType::Text,
            is_system: false,
            storage: FieldStorage::Json,
            is_required: false,
            is_visible: true,
            is_readonly: false,
            is_searchable: false,
            is_unique,
            default_value: None,
            validation: None,
            options: None,
            decimal_places: None,
            relation_entity_id: None,
            formula: None,
            formula_result_type: None,
            ui_widget: None,
            column_width: None,
            help_text: None,
            sort_order: 0,
            is_active: true,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            version: 1,
        }
    }

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn unknown_details_lists_keys_not_in_details() {
        let parts = entity("parts");
        let lines = BTreeMap::from([
            ("parts".to_owned(), vec![]),
            ("zzz".to_owned(), vec![input(None)]),
            ("labor".to_owned(), vec![]),
        ]);
        assert_eq!(
            unknown_details(&lines, &[&parts]),
            vec![
                LineError::detail("labor", MSG_UNKNOWN_DETAIL),
                LineError::detail("zzz", MSG_UNKNOWN_DETAIL),
            ]
        );
        assert!(unknown_details(&BTreeMap::new(), &[]).is_empty());
    }

    #[test]
    fn match_lines_updates_inserts_and_removes() {
        let (a, b, c) = (new_id(), new_id(), new_id());
        let m = match_lines(
            "parts",
            &[input(Some(c)), input(None), input(Some(a))],
            &[a, b, c],
            false,
        )
        .unwrap();
        assert_eq!(
            m.targets,
            vec![
                LineTarget::Existing(2),
                LineTarget::New,
                LineTarget::Existing(0)
            ]
        );
        assert_eq!(m.removed, vec![1]);

        // 空陣列：全部刪除
        let m = match_lines("parts", &[], &[a, b], false).unwrap();
        assert_eq!(m.targets, vec![]);
        assert_eq!(m.removed, vec![0, 1]);
    }

    #[test]
    fn match_lines_rejects_unknown_duplicate_and_create_ids() {
        let (a, other) = (new_id(), new_id());
        assert_eq!(
            match_lines(
                "parts",
                &[
                    input(Some(a)),
                    input(None),
                    input(Some(other)),
                    input(Some(a))
                ],
                &[a],
                false,
            )
            .unwrap_err(),
            vec![
                LineError::row("parts", 2, MSG_LINE_NOT_FOUND),
                LineError::row("parts", 3, MSG_DUPLICATE_LINE),
            ]
        );
        assert_eq!(
            match_lines("parts", &[input(None), input(Some(a))], &[], true).unwrap_err(),
            vec![LineError::row("parts", 1, MSG_ID_ON_CREATE)]
        );
        let m = match_lines("parts", &[input(None), input(None)], &[], true).unwrap();
        assert_eq!(m.targets, vec![LineTarget::New, LineTarget::New]);
        assert!(m.removed.is_empty());
    }

    #[test]
    fn duplicate_line_values_checks_unique_fields_only() {
        let mut inactive = field("old", true);
        inactive.is_active = false;
        let fields = [field("sn", true), field("name", false), inactive];
        let l0 = obj(json!({ "sn": "A", "name": "x", "old": "1" }));
        let l1 = obj(json!({ "sn": "B", "name": "x", "old": "1" }));
        let l2 = obj(json!({ "sn": "A" }));
        let l3 = obj(json!({ "name": "y" }));
        let l4 = obj(json!({ "sn": "B" }));
        assert_eq!(
            duplicate_line_values(
                "parts",
                &fields,
                &[Some(&l0), Some(&l1), None, Some(&l2), Some(&l3), Some(&l4)],
            ),
            vec![
                LineError::field("parts", 3, "sn", MSG_DUPLICATE_VALUE),
                LineError::field("parts", 5, "sn", MSG_DUPLICATE_VALUE),
            ]
        );
    }

    #[test]
    fn duplicate_attachments_across_master_and_lines() {
        let (a, b, c) = (new_id(), new_id(), new_id());
        let fv = |key: &str, id| FileValue {
            field_key: key.into(),
            id,
        };
        let master = [fv("photo", a)];
        let l0 = [fv("img", b)];
        let l1 = [fv("img", a), fv("doc", c)];
        let l2 = [fv("img", b)];
        let errors = duplicate_attachments(
            &master,
            &[
                LineFiles {
                    detail_key: "parts",
                    index: 0,
                    bind: &l0,
                },
                LineFiles {
                    detail_key: "parts",
                    index: 1,
                    bind: &l1,
                },
                LineFiles {
                    detail_key: "labor",
                    index: 0,
                    bind: &l2,
                },
            ],
        );
        assert_eq!(
            errors,
            vec![
                LineError::field("parts", 1, "img", MSG_ATTACHMENT_IN_USE),
                LineError::field("labor", 0, "img", MSG_ATTACHMENT_IN_USE),
            ]
        );
        assert!(duplicate_attachments(&[], &[]).is_empty());
    }

    #[test]
    fn audit_shape() {
        let id = new_id();
        let data = json!({ "qty": 2 });
        assert_eq!(audit_key("parts"), "lines.parts");
        assert_eq!(
            audit_value([(id, &data)]),
            json!([{ "id": id, "data": { "qty": 2 } }])
        );
        assert_eq!(audit_value([]), json!([]));
    }

    #[test]
    fn validation_error_merges_master_and_lines() {
        let master_ok = Ok(obj(json!({ "a": 1 })));
        assert_eq!(
            validation_error(master_ok.clone(), vec![]).unwrap(),
            obj(json!({ "a": 1 }))
        );
        let field_error = FieldError {
            field_key: "name".into(),
            message: "此欄位為必填".into(),
        };
        assert!(matches!(
            validation_error(Err(vec![field_error.clone()]), vec![]),
            Err(CoreError::InvalidFields(f)) if f == vec![field_error.clone()]
        ));
        let line = LineError::field("parts", 0, "qty", "必須是整數");
        assert!(matches!(
            validation_error(master_ok, vec![line.clone()]),
            Err(CoreError::InvalidLines { fields, lines }) if fields.is_empty() && lines == vec![line.clone()]
        ));
        assert!(matches!(
            validation_error(Err(vec![field_error.clone()]), vec![line.clone()]),
            Err(CoreError::InvalidLines { fields, lines }) if fields == vec![field_error] && lines == vec![line]
        ));
    }

    #[test]
    fn from_fields_keeps_row_and_field() {
        let errors: Vec<_> = LineError::from_fields(
            "parts",
            2,
            vec![FieldError {
                field_key: "qty".into(),
                message: "必須是整數".into(),
            }],
        )
        .collect();
        assert_eq!(
            errors,
            vec![LineError::field("parts", 2, "qty", "必須是整數")]
        );
    }
}

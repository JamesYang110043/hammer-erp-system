//! 關聯欄位（`relation`，docs/architecture.md §7.3、§8.5 `field_definitions`）。
//!
//! - 寫入：[`check_relations`] 檢查有變動的關聯欄位所指的資料存在於目標資料類別且未刪除，
//!   並以 `FOR SHARE` 鎖定，避免在寫入的 transaction 結束前被刪除
//! - 回應：[`attach_relations`] 填入 [`Record::relations`]（目標資料的標題與是否已刪除）
//!
//! 純邏輯（[`changed_relations`]、[`missing_relations`]、[`relation_values`]）有單元測試；
//! 存取資料庫的部分由 `crates/server/tests/relation_api.rs` 的整合測試涵蓋。
//!
//! 欄位定義的目標資料類別限制（自訂、有效、`master` / `document`）在
//! [`crate::schema::rules::validate_relation_target`]。

use std::collections::{BTreeMap, HashMap, HashSet};

use hammer_db::record as record_db;
use hammer_db::schema as schema_db;
use hammer_db::sqlx::{PgConnection, PgTransaction};
use hammer_shared::Id;
use hammer_shared::record::{Record, RelationRef};
use hammer_shared::schema::{DataType, FieldDefinition, FieldValue, parse_value};
use serde_json::{Map, Value};

use crate::CoreError;
use crate::record_data::FieldError;
use crate::schema::{entity_from_row, field_from_row};
use crate::title::TitleRenderer;

/// 關聯的資料不存在、已刪除或屬於其他資料類別時的錯誤訊息。
pub const MSG_RELATION_NOT_FOUND: &str = "關聯的資料不存在或已刪除";
/// 關聯欄位沒有設定目標資料類別（資料庫內容不一致）時的錯誤訊息。
pub const MSG_RELATION_NO_TARGET: &str = "此關聯欄位沒有設定關聯的資料類別，請聯絡管理者";

/// 一個關聯欄位的值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationValue {
    pub field_key: String,
    /// 目標資料類別（欄位定義的 `relation_entity_id`）。
    pub entity_id: Id,
    /// 目標資料的 id。
    pub id: Id,
}

/// `data` 中每個有值的 `relation` 欄位（含已停用的欄位），依 `fields` 的順序。
///
/// 沒有設定目標資料類別、或值不是 UUID 的欄位略過（讀取時不因資料庫內容不一致而失敗）。
pub fn relation_values(
    fields: &[FieldDefinition],
    data: &Map<String, Value>,
) -> Vec<RelationValue> {
    fields
        .iter()
        .filter(|f| f.data_type == DataType::Relation)
        .filter_map(|f| {
            let entity_id = f.relation_entity_id?;
            let id = relation_id(data.get(&f.field_key)?)?;
            Some(RelationValue {
                field_key: f.field_key.clone(),
                entity_id,
                id,
            })
        })
        .collect()
}

/// 寫入時要檢查的關聯：`data`（正規化後的完整資料）中有值，且與 `existing`（修改前的資料；
/// 新增時為 `None`）不同的 `relation` 欄位，依 `fields` 的順序。
///
/// 沒有變動的關聯不檢查：目標資料之後被刪除時，仍可修改這筆資料的其他欄位。
/// 欄位沒有設定目標資料類別 → 該欄位的錯誤。
pub fn changed_relations(
    fields: &[FieldDefinition],
    data: &Map<String, Value>,
    existing: Option<&Map<String, Value>>,
) -> Result<Vec<RelationValue>, Vec<FieldError>> {
    let mut values = Vec::new();
    let mut errors = Vec::new();
    for field in fields.iter().filter(|f| f.data_type == DataType::Relation) {
        let key = field.field_key.as_str();
        let Some(value) = data.get(key) else {
            continue;
        };
        if existing.and_then(|e| e.get(key)) == Some(value) {
            continue;
        }
        let Some(entity_id) = field.relation_entity_id else {
            errors.push(field_error(key, MSG_RELATION_NO_TARGET));
            continue;
        };
        // 已經過 normalize_record_data，格式一定正確
        let Some(id) = relation_id(value) else {
            errors.push(field_error(key, MSG_RELATION_NOT_FOUND));
            continue;
        };
        values.push(RelationValue {
            field_key: key.to_owned(),
            entity_id,
            id,
        });
    }
    if errors.is_empty() {
        Ok(values)
    } else {
        Err(errors)
    }
}

/// `values` 中，`(資料類別, id)` 不在 `found` 內的關聯，轉成欄位錯誤（依 `values` 的順序）。
pub fn missing_relations(values: &[RelationValue], found: &HashSet<(Id, Id)>) -> Vec<FieldError> {
    values
        .iter()
        .filter(|v| !found.contains(&(v.entity_id, v.id)))
        .map(|v| field_error(&v.field_key, MSG_RELATION_NOT_FOUND))
        .collect()
}

/// 檢查有變動的關聯欄位（[`changed_relations`]）所指的資料存在於目標資料類別且未刪除，
/// 並以 `FOR SHARE` 鎖定到 transaction 結束。有任何一個不符合 → [`CoreError::InvalidFields`]。
pub async fn check_relations(
    tx: &mut PgTransaction<'_>,
    fields: &[FieldDefinition],
    data: &Map<String, Value>,
    existing: Option<&Map<String, Value>>,
) -> Result<(), CoreError> {
    let values = changed_relations(fields, data, existing).map_err(CoreError::InvalidFields)?;
    if values.is_empty() {
        return Ok(());
    }
    let mut by_entity: BTreeMap<Id, Vec<Id>> = BTreeMap::new();
    for v in &values {
        by_entity.entry(v.entity_id).or_default().push(v.id);
    }
    let mut found = HashSet::new();
    // 依資料類別 id、資料 id 的順序鎖定，降低互相等待的機會
    for (entity_id, mut ids) in by_entity {
        ids.sort();
        ids.dedup();
        for id in record_db::lock_existing_records(&mut **tx, entity_id, &ids).await? {
            found.insert((entity_id, id));
        }
    }
    let errors = missing_relations(&values, &found);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(CoreError::InvalidFields(errors))
    }
}

/// 依 `fields`（資料所屬資料類別的欄位定義）填入每筆資料的 [`Record::relations`]。
///
/// 目標資料依目標資料類別的標題規則產生標題（含已刪除的資料）；找不到目標資料或目標資料類別時，
/// `title` 為空字串、`deleted` 為 `true`。
pub async fn attach_relations(
    conn: &mut PgConnection,
    fields: &[FieldDefinition],
    records: &mut [Record],
) -> Result<(), CoreError> {
    let per_record: Vec<Vec<RelationValue>> = records
        .iter()
        .map(|r| relation_values(fields, &r.data))
        .collect();
    let mut by_entity: BTreeMap<Id, Vec<Id>> = BTreeMap::new();
    for v in per_record.iter().flatten() {
        by_entity.entry(v.entity_id).or_default().push(v.id);
    }
    if by_entity.is_empty() {
        return Ok(());
    }

    let mut resolved: HashMap<(Id, Id), (String, bool)> = HashMap::new();
    for (entity_id, mut ids) in by_entity {
        ids.sort();
        ids.dedup();
        let Some(entity) = schema_db::find_entity_by_id(&mut *conn, entity_id).await? else {
            continue;
        };
        let entity = entity_from_row(entity)?;
        let target_fields = schema_db::list_fields(&mut *conn, entity_id)
            .await?
            .into_iter()
            .map(field_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        let titles = TitleRenderer::new(entity.title_template.as_deref(), &target_fields);
        for row in record_db::find_records_by_ids(&mut *conn, entity_id, &ids).await? {
            let title = match row.data.as_object() {
                Some(data) => titles.render(row.id, row.record_no.as_deref(), data),
                None => String::new(),
            };
            resolved.insert((entity_id, row.id), (title, row.deleted));
        }
    }

    for (record, values) in records.iter_mut().zip(per_record) {
        record.relations = values
            .into_iter()
            .map(|v| {
                let (title, deleted) = resolved
                    .get(&(v.entity_id, v.id))
                    .cloned()
                    .unwrap_or_else(|| (String::new(), true));
                (
                    v.field_key,
                    RelationRef {
                        id: v.id,
                        title,
                        deleted,
                    },
                )
            })
            .collect();
    }
    Ok(())
}

fn relation_id(value: &Value) -> Option<Id> {
    match parse_value(DataType::Relation, value) {
        Ok(Some(FieldValue::Relation(id))) => Some(id),
        _ => None,
    }
}

fn field_error(field_key: &str, message: &str) -> FieldError {
    FieldError {
        field_key: field_key.to_owned(),
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hammer_shared::new_id;
    use hammer_shared::schema::FieldStorage;
    use serde_json::json;
    use time::OffsetDateTime;

    fn field(key: &str, data_type: DataType, target: Option<Id>) -> FieldDefinition {
        FieldDefinition {
            id: new_id(),
            entity_id: Id::nil(),
            field_key: key.to_owned(),
            label: key.to_owned(),
            data_type,
            is_system: false,
            storage: FieldStorage::Json,
            is_required: false,
            is_visible: true,
            is_readonly: false,
            is_searchable: false,
            is_unique: false,
            default_value: None,
            validation: None,
            options: None,
            decimal_places: None,
            relation_entity_id: target,
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

    fn map(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn relation_values_lists_relation_fields_with_values() {
        let (a, b) = (new_id(), new_id());
        let (x, y) = (new_id(), new_id());
        let mut inactive = field("old", DataType::Relation, Some(b));
        inactive.is_active = false;
        let fields = [
            field("name", DataType::Text, None),
            field("equipment", DataType::Relation, Some(a)),
            field("empty", DataType::Relation, Some(a)),
            inactive,
            field("broken", DataType::Relation, None),
        ];
        let data = map(json!({
            "name": "x",
            "equipment": x.to_string(),
            "old": y.to_string(),
            "broken": x.to_string(),
        }));
        assert_eq!(
            relation_values(&fields, &data),
            [
                RelationValue {
                    field_key: "equipment".into(),
                    entity_id: a,
                    id: x,
                },
                RelationValue {
                    field_key: "old".into(),
                    entity_id: b,
                    id: y,
                },
            ]
        );
        // 值不是 UUID 時略過
        let data = map(json!({ "equipment": "abc" }));
        assert!(relation_values(&fields, &data).is_empty());
    }

    #[test]
    fn changed_relations_skips_unchanged_values() {
        let target = new_id();
        let (x, y) = (new_id(), new_id());
        let fields = [
            field("a", DataType::Relation, Some(target)),
            field("b", DataType::Relation, Some(target)),
            field("note", DataType::Text, None),
        ];
        let data = map(json!({ "a": x.to_string(), "b": y.to_string(), "note": "n" }));

        // 新增：全部檢查
        let all = changed_relations(&fields, &data, None).unwrap();
        assert_eq!(
            all.iter().map(|v| v.field_key.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );

        // 修改：只檢查有變動的
        let existing = map(json!({ "a": x.to_string(), "b": x.to_string() }));
        let changed = changed_relations(&fields, &data, Some(&existing)).unwrap();
        assert_eq!(
            changed,
            [RelationValue {
                field_key: "b".into(),
                entity_id: target,
                id: y,
            }]
        );

        // 清空關聯不需檢查
        let cleared = map(json!({ "note": "n" }));
        assert!(
            changed_relations(&fields, &cleared, Some(&existing))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn changed_relations_requires_target_entity() {
        let fields = [field("a", DataType::Relation, None)];
        let data = map(json!({ "a": new_id().to_string() }));
        let errors = changed_relations(&fields, &data, None).unwrap_err();
        assert_eq!(errors, [field_error("a", MSG_RELATION_NO_TARGET)]);
    }

    #[test]
    fn missing_relations_reports_each_field() {
        let (e1, e2) = (new_id(), new_id());
        let (x, y) = (new_id(), new_id());
        let values = [
            RelationValue {
                field_key: "a".into(),
                entity_id: e1,
                id: x,
            },
            RelationValue {
                field_key: "b".into(),
                entity_id: e2,
                id: x,
            },
            RelationValue {
                field_key: "c".into(),
                entity_id: e1,
                id: y,
            },
        ];
        // x 屬於 e1，不屬於 e2；y 不存在
        let found = HashSet::from([(e1, x)]);
        assert_eq!(
            missing_relations(&values, &found),
            [
                field_error("b", MSG_RELATION_NOT_FOUND),
                field_error("c", MSG_RELATION_NOT_FOUND),
            ]
        );
        assert!(missing_relations(&values, &HashSet::from([(e1, x), (e2, x), (e1, y)])).is_empty());
    }
}

//! 資料類別與欄位定義的輸入檢查（純邏輯，不存取資料庫）。
//!
//! - [`plan_create_entity`]、[`plan_update_entity`]：資料類別
//! - [`plan_create_field`]、[`plan_update_field`]：欄位，含 §7.2 系統欄位的限制
//! - [`validate_field_config`]：欄位設定與型態相符（小數位數、選項、驗證規則、預設值）
//! - [`plan_reorder`]：調整欄位排序
//!
//! 錯誤都是 [`CoreError::InvalidField`]，`field` 為 request 的欄位名稱（[`field`]）。

use std::collections::{HashMap, HashSet};

use hammer_shared::schema::{
    CreateEntityRequest, CreateFieldRequest, DataType, EntityDefinition, EntityKind, EntityStorage,
    FieldDefinition, FieldOrderItem, FieldStorage, UpdateEntityRequest, UpdateFieldRequest,
    validate_key,
};
use hammer_shared::{Id, new_id};
use serde_json::Value;
use time::OffsetDateTime;

use crate::CoreError;
use crate::record_data::normalize_field_value;

/// 驗證錯誤 `details.field` 的欄位名稱，與 request 的欄位名稱相同。
pub mod field {
    pub const KEY: &str = "key";
    pub const LABEL: &str = "label";
    pub const KIND: &str = "kind";
    pub const IS_ACTIVE: &str = "is_active";
    pub const FIELD_KEY: &str = "field_key";
    pub const DATA_TYPE: &str = "data_type";
    pub const IS_REQUIRED: &str = "is_required";
    pub const IS_READONLY: &str = "is_readonly";
    pub const IS_SEARCHABLE: &str = "is_searchable";
    pub const IS_UNIQUE: &str = "is_unique";
    pub const DEFAULT_VALUE: &str = "default_value";
    pub const VALIDATION: &str = "validation";
    pub const OPTIONS: &str = "options";
    pub const DECIMAL_PLACES: &str = "decimal_places";
    pub const UI_WIDGET: &str = "ui_widget";
    pub const COLUMN_WIDTH: &str = "column_width";
    pub const FIELDS: &str = "fields";
}

/// `decimal_places` 的上限。資料庫的數量、單價為 `NUMERIC(18,4)`、匯率為 6 位（§8.1）。
pub const MAX_DECIMAL_PLACES: i16 = 6;

/// 新增資料類別通過檢查後的內容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEntityPlan {
    pub key: String,
    pub label: String,
    pub icon: Option<String>,
    pub kind: EntityKind,
    pub storage: EntityStorage,
    pub title_template: Option<String>,
    pub sort_order: i32,
}

/// 檢查新增資料類別的輸入。M2a 只允許 `kind = master`，儲存方式固定為 `custom_records`。
pub fn plan_create_entity(req: &CreateEntityRequest) -> Result<NewEntityPlan, CoreError> {
    validate_key(&req.key).map_err(|e| invalid(field::KEY, e.message()))?;
    let label = validate_label(&req.label)?;
    if req.kind != EntityKind::Master {
        return Err(invalid(
            field::KIND,
            "目前只能建立主檔（master）類型的資料類別",
        ));
    }
    Ok(NewEntityPlan {
        key: req.key.clone(),
        label,
        icon: optional_text(req.icon.as_deref()),
        kind: EntityKind::Master,
        storage: EntityStorage::CustomRecords,
        title_template: optional_text(req.title_template.as_deref()),
        sort_order: req.sort_order,
    })
}

/// 檢查修改資料類別的輸入，回傳修改後的定義（`version` 等不變）。
///
/// - `key` 有帶且與目前不同 → 錯誤（`key`）
/// - 系統資料類別不可停用（`is_active`）
pub fn plan_update_entity(
    current: &EntityDefinition,
    req: &UpdateEntityRequest,
) -> Result<EntityDefinition, CoreError> {
    if let Some(key) = req.key.as_deref()
        && key != current.key
    {
        return Err(invalid(field::KEY, "資料類別代碼建立後不可修改"));
    }
    if current.is_system && !req.is_active {
        return Err(invalid(field::IS_ACTIVE, "系統資料類別不可停用"));
    }
    Ok(EntityDefinition {
        label: validate_label(&req.label)?,
        icon: optional_text(req.icon.as_deref()),
        title_template: optional_text(req.title_template.as_deref()),
        sort_order: req.sort_order,
        is_active: req.is_active,
        ..current.clone()
    })
}

/// 檢查新增欄位的輸入，回傳要寫入的欄位定義（自訂欄位、`storage = json`、啟用中）。
pub fn plan_create_field(
    entity_id: Id,
    req: &CreateFieldRequest,
    now: OffsetDateTime,
) -> Result<FieldDefinition, CoreError> {
    validate_key(&req.field_key).map_err(|e| invalid(field::FIELD_KEY, e.message()))?;
    let def = FieldDefinition {
        id: new_id(),
        entity_id,
        field_key: req.field_key.clone(),
        label: req.label.clone(),
        data_type: req.data_type,
        is_system: false,
        storage: FieldStorage::Json,
        is_required: req.is_required,
        is_visible: req.is_visible,
        is_readonly: req.is_readonly,
        is_searchable: req.is_searchable,
        is_unique: req.is_unique,
        default_value: req.default_value.clone(),
        validation: req.validation.clone(),
        options: req.options.clone(),
        decimal_places: req.decimal_places,
        relation_entity_id: None,
        formula: None,
        formula_result_type: None,
        ui_widget: req.ui_widget.clone(),
        column_width: req.column_width,
        help_text: req.help_text.clone(),
        sort_order: req.sort_order,
        is_active: true,
        created_at: now,
        updated_at: now,
        version: 1,
    };
    validate_field_config(def)
}

/// 檢查修改欄位的輸入，回傳修改後的定義（`version` 等不變）。
///
/// - `field_key` 有帶且與目前不同、`data_type` 與目前不同 → 錯誤（型態變更另有 API）
/// - 自訂欄位：設定必須與型態相符（[`validate_field_config`]）
/// - 系統欄位（§7.2）：只能修改顯示名稱、顯示或隱藏、排序、欄寬、說明文字、必填與預設值；
///   必填不可取消、不可停用，其他屬性必須維持原值
pub fn plan_update_field(
    current: &FieldDefinition,
    req: &UpdateFieldRequest,
) -> Result<FieldDefinition, CoreError> {
    if let Some(key) = req.field_key.as_deref()
        && key != current.field_key
    {
        return Err(invalid(field::FIELD_KEY, "欄位代碼建立後不可修改"));
    }
    if req.data_type != current.data_type {
        let message = if current.is_system {
            "系統欄位不可變更資料型態"
        } else {
            "此操作不可變更資料型態"
        };
        return Err(invalid(field::DATA_TYPE, message));
    }

    let next = FieldDefinition {
        label: req.label.clone(),
        is_required: req.is_required,
        is_visible: req.is_visible,
        is_readonly: req.is_readonly,
        is_searchable: req.is_searchable,
        is_unique: req.is_unique,
        default_value: req.default_value.clone(),
        validation: req.validation.clone(),
        options: req.options.clone(),
        decimal_places: req.decimal_places,
        ui_widget: req.ui_widget.clone(),
        column_width: req.column_width,
        help_text: req.help_text.clone(),
        sort_order: req.sort_order,
        is_active: req.is_active,
        ..current.clone()
    };

    if current.is_system {
        validate_system_field_update(current, next)
    } else {
        validate_field_config(next)
    }
}

/// 系統欄位的修改限制（§7.2）。
fn validate_system_field_update(
    current: &FieldDefinition,
    mut next: FieldDefinition,
) -> Result<FieldDefinition, CoreError> {
    const LOCKED: &str = "系統欄位不可修改此屬性";
    if !next.is_active {
        return Err(invalid(field::IS_ACTIVE, "系統欄位不可停用"));
    }
    if current.is_required && !next.is_required {
        return Err(invalid(field::IS_REQUIRED, "系統欄位不可取消必填"));
    }
    let locked = [
        (field::IS_READONLY, next.is_readonly == current.is_readonly),
        (
            field::IS_SEARCHABLE,
            next.is_searchable == current.is_searchable,
        ),
        (field::IS_UNIQUE, next.is_unique == current.is_unique),
        (field::VALIDATION, next.validation == current.validation),
        (field::OPTIONS, next.options == current.options),
        (
            field::DECIMAL_PLACES,
            next.decimal_places == current.decimal_places,
        ),
        (field::UI_WIDGET, next.ui_widget == current.ui_widget),
    ];
    if let Some((name, _)) = locked.iter().find(|(_, same)| !same) {
        return Err(invalid(name, LOCKED));
    }

    next.label = validate_label(&next.label)?;
    next.help_text = optional_text(next.help_text.as_deref());
    validate_column_width(next.column_width)?;
    if next.default_value != current.default_value {
        next.default_value = normalize_default(&next)?;
    }
    Ok(next)
}

/// 檢查自訂欄位的設定與型態相符，回傳整理後的定義：
///
/// - 型態目前支援（M2a 不允許 `relation`、`file`、`formula`）
/// - 顯示名稱去除前後空白後不可為空；`ui_widget`、`help_text` 空白視為未設定
/// - `decimal_places`：`decimal`、`money` 必填（0 到 [`MAX_DECIMAL_PLACES`]），其他型態不可設定
/// - `options`：`select`、`multi_select` 必填且至少一個，`value` 不可空白或重複、`label` 不可空白；
///   其他型態不可設定
/// - `validation`：`min`、`max` 只用於 `integer`、`decimal`、`money`，且 `min <= max`；
///   `max_length` 只用於 `text`、`long_text`，且大於 0；`pattern` 只用於 `text`，且必須是正確的
///   正規表示式。沒有任何規則時存成 `null`
/// - `column_width` 大於 0
/// - `default_value` 依 [`normalize_field_value`] 驗證並正規化；`null` 或空值存成 `null`
pub fn validate_field_config(mut def: FieldDefinition) -> Result<FieldDefinition, CoreError> {
    if !def.data_type.is_value_supported() {
        return Err(invalid(field::DATA_TYPE, "目前不支援此欄位型態"));
    }
    def.label = validate_label(&def.label)?;
    def.ui_widget = optional_text(def.ui_widget.as_deref());
    def.help_text = optional_text(def.help_text.as_deref());
    validate_column_width(def.column_width)?;

    let t = def.data_type;
    match (t.uses_decimal_places(), def.decimal_places) {
        (true, None) => return Err(invalid(field::DECIMAL_PLACES, "請設定小數位數")),
        (true, Some(p)) if !(0..=MAX_DECIMAL_PLACES).contains(&p) => {
            return Err(invalid(
                field::DECIMAL_PLACES,
                &format!("小數位數必須介於 0 到 {MAX_DECIMAL_PLACES}"),
            ));
        }
        (false, Some(_)) => return Err(invalid(field::DECIMAL_PLACES, "此型態不使用小數位數")),
        _ => {}
    }

    match (t.uses_options(), def.options.as_mut()) {
        (true, None) => return Err(invalid(field::OPTIONS, "請設定選項")),
        (true, Some(options)) => {
            if options.is_empty() {
                return Err(invalid(field::OPTIONS, "請設定選項"));
            }
            let mut seen = HashSet::new();
            for o in options.iter_mut() {
                o.value = o.value.trim().to_owned();
                o.label = o.label.trim().to_owned();
                o.color = optional_text(o.color.as_deref());
                if o.value.is_empty() || o.label.is_empty() {
                    return Err(invalid(field::OPTIONS, "選項的值與名稱不可空白"));
                }
                if !seen.insert(o.value.clone()) {
                    return Err(invalid(
                        field::OPTIONS,
                        &format!("選項的值「{}」重複", o.value),
                    ));
                }
            }
        }
        (false, Some(_)) => return Err(invalid(field::OPTIONS, "此型態不使用選項")),
        (false, None) => {}
    }

    if let Some(v) = def.validation.as_ref() {
        let numeric = matches!(t, DataType::Integer | DataType::Decimal | DataType::Money);
        let textual = matches!(t, DataType::Text | DataType::LongText);
        if (v.min.is_some() || v.max.is_some()) && !numeric {
            return Err(invalid(field::VALIDATION, "此型態不使用最小值、最大值"));
        }
        if let (Some(min), Some(max)) = (v.min, v.max)
            && min > max
        {
            return Err(invalid(field::VALIDATION, "最小值不可大於最大值"));
        }
        if let Some(max_length) = v.max_length {
            if !textual {
                return Err(invalid(field::VALIDATION, "此型態不使用字元數上限"));
            }
            if max_length == 0 {
                return Err(invalid(field::VALIDATION, "字元數上限必須大於 0"));
            }
        }
        if let Some(pattern) = v.pattern.as_deref() {
            if t != DataType::Text {
                return Err(invalid(field::VALIDATION, "此型態不使用格式規則"));
            }
            if regex::Regex::new(pattern).is_err() {
                return Err(invalid(field::VALIDATION, "格式規則不是正確的正規表示式"));
            }
        }
        if *v == Default::default() {
            def.validation = None;
        }
    }

    def.default_value = normalize_default(&def)?;
    Ok(def)
}

/// 以欄位的設定驗證並正規化預設值。
fn normalize_default(def: &FieldDefinition) -> Result<Option<Value>, CoreError> {
    match def.default_value.as_ref() {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            if !def.data_type.is_value_supported() {
                return Err(invalid(field::DEFAULT_VALUE, "此型態目前不支援預設值"));
            }
            normalize_field_value(def, value)
                .map_err(|message| invalid(field::DEFAULT_VALUE, &format!("預設值{message}")))
        }
    }
}

/// 檢查調整排序的輸入，回傳每個欄位 id 的新 `sort_order` 與輸入的版本號。
///
/// `current` 為該資料類別所有未刪除的欄位。輸入必須恰好列出每個欄位一次。
pub fn plan_reorder(
    current: &[FieldDefinition],
    items: &[FieldOrderItem],
) -> Result<Vec<ReorderStep>, CoreError> {
    let by_key: HashMap<&str, &FieldDefinition> =
        current.iter().map(|f| (f.field_key.as_str(), f)).collect();
    let mut seen = HashSet::new();
    let mut steps = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let def = by_key
            .get(item.field_key.as_str())
            .ok_or_else(|| invalid(field::FIELDS, &format!("找不到欄位「{}」", item.field_key)))?;
        if !seen.insert(item.field_key.as_str()) {
            return Err(invalid(
                field::FIELDS,
                &format!("欄位「{}」重複", item.field_key),
            ));
        }
        let sort_order =
            i32::try_from(index).map_err(|_| invalid(field::FIELDS, "欄位數量過多"))?;
        steps.push(ReorderStep {
            field: (*def).clone(),
            version: item.version,
            sort_order,
        });
    }
    if steps.len() != current.len() {
        return Err(invalid(
            field::FIELDS,
            "必須依新的順序列出所有欄位（含已停用）",
        ));
    }
    Ok(steps)
}

/// 調整排序時一個欄位的變更。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReorderStep {
    /// 目前的定義。
    pub field: FieldDefinition,
    /// 輸入的版本號。
    pub version: i32,
    /// 新的排序。
    pub sort_order: i32,
}

impl ReorderStep {
    /// 排序是否改變。
    pub fn changes(&self) -> bool {
        self.field.sort_order != self.sort_order
    }
}

fn validate_label(label: &str) -> Result<String, CoreError> {
    let label = label.trim();
    if label.is_empty() {
        return Err(invalid(field::LABEL, "請輸入顯示名稱"));
    }
    Ok(label.to_owned())
}

fn validate_column_width(width: Option<i32>) -> Result<(), CoreError> {
    match width {
        Some(w) if w <= 0 => Err(invalid(field::COLUMN_WIDTH, "欄寬必須大於 0")),
        _ => Ok(()),
    }
}

/// 去除前後空白；空字串視為未設定。
fn optional_text(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn invalid(field: &'static str, message: &str) -> CoreError {
    CoreError::InvalidField {
        field,
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hammer_shared::ErrorCode;
    use hammer_shared::decimal::Decimal;
    use hammer_shared::schema::{SelectOption, Validation};
    use serde_json::json;
    use time::macros::datetime;

    const NOW: OffsetDateTime = datetime!(2026-11-13 09:00 +8);

    fn field_of(err: &CoreError) -> &'static str {
        match err {
            CoreError::InvalidField { field, .. } => field,
            other => panic!("不是欄位錯誤：{other:?}"),
        }
    }

    fn create_req(key: &str, data_type: DataType) -> CreateFieldRequest {
        CreateFieldRequest {
            field_key: key.into(),
            label: "名稱".into(),
            data_type,
            is_required: false,
            is_visible: true,
            is_readonly: false,
            is_searchable: false,
            is_unique: false,
            default_value: None,
            validation: None,
            options: None,
            decimal_places: None,
            ui_widget: None,
            column_width: None,
            help_text: None,
            sort_order: 0,
        }
    }

    fn update_req(def: &FieldDefinition) -> UpdateFieldRequest {
        UpdateFieldRequest {
            version: def.version,
            field_key: None,
            label: def.label.clone(),
            data_type: def.data_type,
            is_required: def.is_required,
            is_visible: def.is_visible,
            is_readonly: def.is_readonly,
            is_searchable: def.is_searchable,
            is_unique: def.is_unique,
            default_value: def.default_value.clone(),
            validation: def.validation.clone(),
            options: def.options.clone(),
            decimal_places: def.decimal_places,
            ui_widget: def.ui_widget.clone(),
            column_width: def.column_width,
            help_text: def.help_text.clone(),
            sort_order: def.sort_order,
            is_active: def.is_active,
        }
    }

    fn created(req: &CreateFieldRequest) -> FieldDefinition {
        plan_create_field(new_id(), req, NOW).unwrap()
    }

    fn create_err(req: &CreateFieldRequest) -> &'static str {
        let err = plan_create_field(new_id(), req, NOW).unwrap_err();
        assert_eq!(err.code(), ErrorCode::Validation);
        field_of(&err)
    }

    fn options(values: &[&str]) -> Option<Vec<SelectOption>> {
        Some(
            values
                .iter()
                .map(|v| SelectOption {
                    value: (*v).into(),
                    label: v.to_uppercase(),
                    color: None,
                })
                .collect(),
        )
    }

    fn system_field() -> FieldDefinition {
        let mut def = created(&CreateFieldRequest {
            is_required: true,
            ..create_req("name", DataType::Text)
        });
        def.is_system = true;
        def.storage = FieldStorage::Column;
        def
    }

    fn entity(is_system: bool) -> EntityDefinition {
        EntityDefinition {
            id: new_id(),
            key: "repair_order".into(),
            label: "維修單".into(),
            icon: None,
            kind: EntityKind::Master,
            is_system,
            storage: EntityStorage::CustomRecords,
            parent_entity_id: None,
            numbering_pattern: None,
            title_template: None,
            sort_order: 0,
            is_active: true,
            created_at: NOW,
            updated_at: NOW,
            version: 3,
        }
    }

    fn entity_update(e: &EntityDefinition) -> UpdateEntityRequest {
        UpdateEntityRequest {
            version: e.version,
            key: None,
            label: e.label.clone(),
            icon: e.icon.clone(),
            title_template: e.title_template.clone(),
            sort_order: e.sort_order,
            is_active: e.is_active,
        }
    }

    // ---- 資料類別 ----

    #[test]
    fn create_entity_normalizes_input() {
        let plan = plan_create_entity(&CreateEntityRequest {
            key: "repair_order".into(),
            label: " 維修單 ".into(),
            icon: Some("  ".into()),
            kind: EntityKind::Master,
            title_template: Some(" {name} ".into()),
            sort_order: 5,
        })
        .unwrap();
        assert_eq!(plan.label, "維修單");
        assert_eq!(plan.icon, None);
        assert_eq!(plan.title_template.as_deref(), Some("{name}"));
        assert_eq!(plan.storage, EntityStorage::CustomRecords);
        assert_eq!(plan.sort_order, 5);
    }

    #[test]
    fn create_entity_rejects_invalid_input() {
        let base = CreateEntityRequest {
            key: "repair_order".into(),
            label: "維修單".into(),
            icon: None,
            kind: EntityKind::Master,
            title_template: None,
            sort_order: 0,
        };
        let cases = [
            (
                CreateEntityRequest {
                    key: "Repair".into(),
                    ..base.clone()
                },
                field::KEY,
            ),
            (
                CreateEntityRequest {
                    label: " ".into(),
                    ..base.clone()
                },
                field::LABEL,
            ),
            (
                CreateEntityRequest {
                    kind: EntityKind::Document,
                    ..base.clone()
                },
                field::KIND,
            ),
            (
                CreateEntityRequest {
                    kind: EntityKind::Detail,
                    ..base.clone()
                },
                field::KIND,
            ),
        ];
        for (req, expected) in cases {
            assert_eq!(field_of(&plan_create_entity(&req).unwrap_err()), expected);
        }
    }

    #[test]
    fn update_entity_merges_editable_fields() {
        let current = entity(false);
        let next = plan_update_entity(
            &current,
            &UpdateEntityRequest {
                label: " 維修 ".into(),
                icon: Some("tool".into()),
                sort_order: 9,
                is_active: false,
                key: Some(current.key.clone()),
                ..entity_update(&current)
            },
        )
        .unwrap();
        assert_eq!(next.label, "維修");
        assert_eq!(next.icon.as_deref(), Some("tool"));
        assert_eq!(next.sort_order, 9);
        assert!(!next.is_active);
        assert_eq!(next.key, current.key);
        assert_eq!(next.version, current.version);
    }

    #[test]
    fn update_entity_rejects_key_change_and_system_deactivation() {
        let current = entity(false);
        let err = plan_update_entity(
            &current,
            &UpdateEntityRequest {
                key: Some("other".into()),
                ..entity_update(&current)
            },
        )
        .unwrap_err();
        assert_eq!(field_of(&err), field::KEY);

        let system = entity(true);
        let err = plan_update_entity(
            &system,
            &UpdateEntityRequest {
                is_active: false,
                ..entity_update(&system)
            },
        )
        .unwrap_err();
        assert_eq!(field_of(&err), field::IS_ACTIVE);
        // 系統資料類別可以改名稱
        assert!(
            plan_update_entity(
                &system,
                &UpdateEntityRequest {
                    label: "客戶".into(),
                    ..entity_update(&system)
                }
            )
            .is_ok()
        );
    }

    // ---- 新增欄位 ----

    #[test]
    fn create_field_sets_defaults() {
        let entity_id = new_id();
        let def = plan_create_field(
            entity_id,
            &CreateFieldRequest {
                label: " 備註 ".into(),
                help_text: Some(" ".into()),
                ..create_req("note", DataType::LongText)
            },
            NOW,
        )
        .unwrap();
        assert_eq!(def.entity_id, entity_id);
        assert_eq!(def.label, "備註");
        assert_eq!(def.help_text, None);
        assert_eq!(def.storage, FieldStorage::Json);
        assert!(!def.is_system && def.is_active);
        assert_eq!(def.version, 1);
    }

    #[test]
    fn create_field_rejects_invalid_key_and_unsupported_types() {
        assert_eq!(
            create_err(&create_req("1abc", DataType::Text)),
            field::FIELD_KEY
        );
        for t in [DataType::Relation, DataType::File, DataType::Formula] {
            assert_eq!(create_err(&create_req("x", t)), field::DATA_TYPE);
        }
        assert_eq!(
            create_err(&CreateFieldRequest {
                label: "".into(),
                ..create_req("x", DataType::Text)
            }),
            field::LABEL
        );
        assert_eq!(
            create_err(&CreateFieldRequest {
                column_width: Some(0),
                ..create_req("x", DataType::Text)
            }),
            field::COLUMN_WIDTH
        );
    }

    #[test]
    fn decimal_places_rules() {
        assert_eq!(
            create_err(&create_req("amount", DataType::Money)),
            field::DECIMAL_PLACES
        );
        assert_eq!(
            create_err(&CreateFieldRequest {
                decimal_places: Some(7),
                ..create_req("amount", DataType::Decimal)
            }),
            field::DECIMAL_PLACES
        );
        assert_eq!(
            create_err(&CreateFieldRequest {
                decimal_places: Some(2),
                ..create_req("qty", DataType::Integer)
            }),
            field::DECIMAL_PLACES
        );
        let def = created(&CreateFieldRequest {
            decimal_places: Some(0),
            ..create_req("amount", DataType::Money)
        });
        assert_eq!(def.decimal_places, Some(0));
    }

    #[test]
    fn options_rules() {
        assert_eq!(
            create_err(&create_req("status", DataType::Select)),
            field::OPTIONS
        );
        assert_eq!(
            create_err(&CreateFieldRequest {
                options: Some(vec![]),
                ..create_req("status", DataType::MultiSelect)
            }),
            field::OPTIONS
        );
        assert_eq!(
            create_err(&CreateFieldRequest {
                options: options(&["a", " a "]),
                ..create_req("status", DataType::Select)
            }),
            field::OPTIONS
        );
        assert_eq!(
            create_err(&CreateFieldRequest {
                options: options(&[" "]),
                ..create_req("status", DataType::Select)
            }),
            field::OPTIONS
        );
        assert_eq!(
            create_err(&CreateFieldRequest {
                options: options(&["a"]),
                ..create_req("name", DataType::Text)
            }),
            field::OPTIONS
        );
        let def = created(&CreateFieldRequest {
            options: Some(vec![SelectOption {
                value: " open ".into(),
                label: " 處理中 ".into(),
                color: Some("".into()),
            }]),
            ..create_req("status", DataType::Select)
        });
        assert_eq!(
            def.options,
            Some(vec![SelectOption {
                value: "open".into(),
                label: "處理中".into(),
                color: None,
            }])
        );
    }

    #[test]
    fn validation_rules_match_type() {
        let range = Validation {
            min: Some(Decimal::from(10)),
            max: Some(Decimal::from(1)),
            ..Default::default()
        };
        assert_eq!(
            create_err(&CreateFieldRequest {
                validation: Some(range),
                ..create_req("qty", DataType::Integer)
            }),
            field::VALIDATION
        );
        let min_only = Validation {
            min: Some(Decimal::from(0)),
            ..Default::default()
        };
        assert_eq!(
            create_err(&CreateFieldRequest {
                validation: Some(min_only.clone()),
                ..create_req("name", DataType::Text)
            }),
            field::VALIDATION
        );
        assert!(
            plan_create_field(
                new_id(),
                &CreateFieldRequest {
                    validation: Some(min_only),
                    ..create_req("qty", DataType::Integer)
                },
                NOW
            )
            .is_ok()
        );
        for (t, v) in [
            (
                DataType::Integer,
                Validation {
                    max_length: Some(5),
                    ..Default::default()
                },
            ),
            (
                DataType::Text,
                Validation {
                    max_length: Some(0),
                    ..Default::default()
                },
            ),
            (
                DataType::LongText,
                Validation {
                    pattern: Some("^a$".into()),
                    ..Default::default()
                },
            ),
            (
                DataType::Text,
                Validation {
                    pattern: Some("(".into()),
                    ..Default::default()
                },
            ),
        ] {
            assert_eq!(
                create_err(&CreateFieldRequest {
                    validation: Some(v),
                    ..create_req("x", t)
                }),
                field::VALIDATION,
                "{t:?}"
            );
        }
        // 沒有任何規則時存成 null
        let def = created(&CreateFieldRequest {
            validation: Some(Validation::default()),
            ..create_req("name", DataType::Text)
        });
        assert_eq!(def.validation, None);
    }

    #[test]
    fn default_value_is_validated_and_normalized() {
        let def = created(&CreateFieldRequest {
            decimal_places: Some(2),
            default_value: Some(json!("1.5")),
            ..create_req("price", DataType::Decimal)
        });
        assert_eq!(def.default_value, Some(json!("1.50")));

        let def = created(&CreateFieldRequest {
            default_value: Some(Value::Null),
            ..create_req("name", DataType::Text)
        });
        assert_eq!(def.default_value, None);

        // 型態不符、不在選項內、超過範圍
        let cases = [
            CreateFieldRequest {
                default_value: Some(json!("abc")),
                ..create_req("qty", DataType::Integer)
            },
            CreateFieldRequest {
                options: options(&["a"]),
                default_value: Some(json!("b")),
                ..create_req("status", DataType::Select)
            },
            CreateFieldRequest {
                validation: Some(Validation {
                    max: Some(Decimal::from(10)),
                    ..Default::default()
                }),
                default_value: Some(json!(11)),
                ..create_req("qty", DataType::Integer)
            },
            CreateFieldRequest {
                decimal_places: Some(1),
                default_value: Some(json!(1.5)),
                ..create_req("price", DataType::Decimal)
            },
        ];
        for req in cases {
            assert_eq!(create_err(&req), field::DEFAULT_VALUE, "{req:?}");
        }
    }

    // ---- 修改欄位 ----

    #[test]
    fn update_custom_field_replaces_attributes() {
        let current = created(&create_req("name", DataType::Text));
        let next = plan_update_field(
            &current,
            &UpdateFieldRequest {
                label: " 品名 ".into(),
                is_required: true,
                is_active: false,
                sort_order: 3,
                default_value: Some(json!("A")),
                ..update_req(&current)
            },
        )
        .unwrap();
        assert_eq!(next.label, "品名");
        assert!(next.is_required && !next.is_active);
        assert_eq!(next.sort_order, 3);
        assert_eq!(next.default_value, Some(json!("A")));
        assert_eq!(next.id, current.id);
        assert_eq!(next.version, current.version);
    }

    #[test]
    fn update_rejects_field_key_and_data_type_change() {
        let current = created(&create_req("name", DataType::Text));
        let err = plan_update_field(
            &current,
            &UpdateFieldRequest {
                field_key: Some("title".into()),
                ..update_req(&current)
            },
        )
        .unwrap_err();
        assert_eq!(field_of(&err), field::FIELD_KEY);
        // 帶相同的代碼可以
        assert!(
            plan_update_field(
                &current,
                &UpdateFieldRequest {
                    field_key: Some("name".into()),
                    ..update_req(&current)
                }
            )
            .is_ok()
        );
        let err = plan_update_field(
            &current,
            &UpdateFieldRequest {
                data_type: DataType::LongText,
                ..update_req(&current)
            },
        )
        .unwrap_err();
        assert_eq!(field_of(&err), field::DATA_TYPE);

        let system = system_field();
        let err = plan_update_field(
            &system,
            &UpdateFieldRequest {
                data_type: DataType::Integer,
                ..update_req(&system)
            },
        )
        .unwrap_err();
        assert_eq!(field_of(&err), field::DATA_TYPE);
        assert!(err.to_string().contains("系統欄位"));
    }

    #[test]
    fn update_custom_field_validates_config() {
        let current = created(&CreateFieldRequest {
            options: options(&["a", "b"]),
            ..create_req("status", DataType::Select)
        });
        let err = plan_update_field(
            &current,
            &UpdateFieldRequest {
                options: None,
                ..update_req(&current)
            },
        )
        .unwrap_err();
        assert_eq!(field_of(&err), field::OPTIONS);
        // 預設值必須在新的選項內
        let err = plan_update_field(
            &current,
            &UpdateFieldRequest {
                default_value: Some(json!("c")),
                ..update_req(&current)
            },
        )
        .unwrap_err();
        assert_eq!(field_of(&err), field::DEFAULT_VALUE);
    }

    #[test]
    fn system_field_allows_display_changes() {
        let current = system_field();
        let next = plan_update_field(
            &current,
            &UpdateFieldRequest {
                label: "客戶名稱".into(),
                is_visible: false,
                sort_order: 7,
                column_width: Some(200),
                help_text: Some("說明".into()),
                default_value: Some(json!("預設")),
                ..update_req(&current)
            },
        )
        .unwrap();
        assert_eq!(next.label, "客戶名稱");
        assert!(!next.is_visible);
        assert_eq!(next.column_width, Some(200));
        assert_eq!(next.default_value, Some(json!("預設")));
        assert!(next.is_system);
    }

    #[test]
    fn system_field_restrictions() {
        let current = system_field();
        let base = update_req(&current);
        let cases = [
            (
                UpdateFieldRequest {
                    is_active: false,
                    ..base.clone()
                },
                field::IS_ACTIVE,
            ),
            (
                UpdateFieldRequest {
                    is_required: false,
                    ..base.clone()
                },
                field::IS_REQUIRED,
            ),
            (
                UpdateFieldRequest {
                    is_readonly: true,
                    ..base.clone()
                },
                field::IS_READONLY,
            ),
            (
                UpdateFieldRequest {
                    is_unique: true,
                    ..base.clone()
                },
                field::IS_UNIQUE,
            ),
            (
                UpdateFieldRequest {
                    is_searchable: true,
                    ..base.clone()
                },
                field::IS_SEARCHABLE,
            ),
            (
                UpdateFieldRequest {
                    validation: Some(Validation {
                        max_length: Some(3),
                        ..Default::default()
                    }),
                    ..base.clone()
                },
                field::VALIDATION,
            ),
            (
                UpdateFieldRequest {
                    ui_widget: Some("textarea".into()),
                    ..base.clone()
                },
                field::UI_WIDGET,
            ),
            (
                UpdateFieldRequest {
                    default_value: Some(json!(1)),
                    ..base.clone()
                },
                field::DEFAULT_VALUE,
            ),
            (
                UpdateFieldRequest {
                    label: " ".into(),
                    ..base.clone()
                },
                field::LABEL,
            ),
        ];
        for (req, expected) in cases {
            let err = plan_update_field(&current, &req).unwrap_err();
            assert_eq!(field_of(&err), expected, "{req:?}");
        }
        // 非必填的系統欄位可以改成必填
        let mut optional = system_field();
        optional.is_required = false;
        assert!(
            plan_update_field(
                &optional,
                &UpdateFieldRequest {
                    is_required: true,
                    ..update_req(&optional)
                }
            )
            .is_ok()
        );
    }

    // ---- 排序 ----

    #[test]
    fn reorder_assigns_positions() {
        let a = created(&create_req("a", DataType::Text));
        let mut b = created(&create_req("b", DataType::Text));
        b.sort_order = 1;
        let steps = plan_reorder(
            &[a.clone(), b.clone()],
            &[
                FieldOrderItem {
                    field_key: "b".into(),
                    version: 4,
                },
                FieldOrderItem {
                    field_key: "a".into(),
                    version: 1,
                },
            ],
        )
        .unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].field.id, b.id);
        assert_eq!((steps[0].sort_order, steps[0].version), (0, 4));
        assert!(steps[0].changes());
        assert_eq!(steps[1].field.id, a.id);
        assert_eq!(steps[1].sort_order, 1);
        assert!(steps[1].changes());
    }

    #[test]
    fn reorder_requires_each_field_once() {
        let a = created(&create_req("a", DataType::Text));
        let b = created(&create_req("b", DataType::Text));
        let current = [a, b];
        let item = |k: &str| FieldOrderItem {
            field_key: k.into(),
            version: 1,
        };
        for items in [
            vec![item("a")],
            vec![item("a"), item("a")],
            vec![item("a"), item("b"), item("c")],
            vec![],
        ] {
            let err = plan_reorder(&current, &items).unwrap_err();
            assert_eq!(field_of(&err), field::FIELDS, "{items:?}");
        }
        // 不變的排序
        let steps = plan_reorder(&current, &[item("a"), item("b")]).unwrap();
        assert!(!steps[0].changes());
        assert!(steps[1].changes());
    }
}

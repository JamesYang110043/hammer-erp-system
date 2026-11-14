//! 依欄位定義驗證並正規化一筆資料的 `data`（`docs/architecture.md` §7.2、§7.3、§8.5）。
//!
//! 伺服器寫入 `custom_records.data` / `custom_data` 前，以及日後的匯入功能，都使用 [`normalize_record_data`]。
//!
//! # 規則
//!
//! - 只處理 `storage = json` 的欄位；`storage = column` 的系統欄位不在 `data` 內，視為未定義
//! - 未定義的 `field_key` 拒絕
//! - 型態依 §7.3 的存值格式（[`hammer_shared::schema::value`]）
//! - `validation`：`integer`、`decimal`、`money` 檢查 `min`、`max`（含）；
//!   `text`、`long_text` 檢查 `max_length`（字元數）；`text` 檢查 `pattern`
//! - `select`、`multi_select` 的值必須在 `options` 內；`multi_select` 不可重複
//! - `decimal`、`money` 的小數位數不可超過 `decimal_places`（不自動四捨五入）；
//!   通過後補零到 `decimal_places` 位，例如 2 位時 `"12.3"` 存成 `"12.30"`
//! - 唯讀（`is_readonly`）與已停用（`is_active = false`）的欄位不可由使用者寫入，帶了就拒絕
//! - 必填（`is_required`）：值不可為 `null`、空字串或空陣列
//! - `relation`、`file`：值必須是 UUID 字串，存成小寫連字號格式；空字串視為沒有值
//!
//! # 新增與更新
//!
//! - 新增（[`WriteMode::Create`]）：沒有帶到的有效欄位套用 `default_value`，
//!   之後檢查所有有效、可寫入欄位的必填
//! - 更新（[`WriteMode::Update`]）：只合併有帶到的欄位，`null` 代表清空；
//!   沒有帶到的欄位（包含已停用的欄位）保留既有的值。必填只檢查有帶到的欄位，
//!   避免新增必填欄位後，舊資料修改其他欄位時被擋下
//!
//! 沒有值（`null`、空字串、空陣列）的欄位不會出現在結果中。
//!
//! # 不處理
//!
//! - 不可重複（`is_unique`）：由資料庫唯一索引處理（§7.5）
//! - `relation` 所指的資料是否存在：需要查詢資料庫，在 [`crate::relations::check_relations`]
//! - `file` 的附件是否存在、是否已被使用：在 [`crate::attachments::prepare_file_changes`]
//! - `formula`（M2c）：寫入時回傳「目前不支援此欄位型態」；更新時沒有帶到則保留既有的值

use hammer_shared::decimal::Decimal;
use hammer_shared::schema::{
    DataType, FieldDefinition, FieldStorage, FieldValue, parse_value, to_json,
};
use serde_json::{Map, Value};

/// 寫入方式。
#[derive(Debug, Clone, Copy)]
pub enum WriteMode<'a> {
    /// 新增資料。
    Create,
    /// 修改既有資料；內容為目前的 `data`。
    Update { existing: &'a Map<String, Value> },
}

/// 一個欄位的驗證錯誤。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldError {
    /// 出錯的欄位代碼。
    pub field_key: String,
    /// 顯示給使用者的說明（繁體中文）。
    pub message: String,
}

impl FieldError {
    fn new(field_key: &str, message: impl Into<String>) -> Self {
        Self {
            field_key: field_key.to_owned(),
            message: message.into(),
        }
    }
}

const MSG_REQUIRED: &str = "此欄位為必填";
const MSG_UNDEFINED: &str = "未定義的欄位";
const MSG_READONLY: &str = "此欄位為唯讀，不可修改";
const MSG_INACTIVE: &str = "此欄位已停用，不可修改";
const MSG_PATTERN: &str = "格式不符合規則";
const MSG_INVALID_PATTERN: &str = "欄位的格式規則設定錯誤，請聯絡管理者";

/// 依欄位定義驗證並正規化 `input`，回傳要寫入的完整 `data`。
///
/// `fields` 為該資料類別的所有欄位定義（包含已停用的）。
/// 有任何錯誤時，回傳所有出錯的欄位：先依 `fields` 的順序，再接未定義的欄位（依鍵排序）。
pub fn normalize_record_data(
    fields: &[FieldDefinition],
    input: &Map<String, Value>,
    mode: WriteMode<'_>,
) -> Result<Map<String, Value>, Vec<FieldError>> {
    let json_fields: Vec<&FieldDefinition> = fields
        .iter()
        .filter(|f| f.storage == FieldStorage::Json)
        .collect();

    let mut errors = Vec::new();
    let mut data = match mode {
        WriteMode::Create => Map::new(),
        WriteMode::Update { existing } => existing.clone(),
    };

    for field in &json_fields {
        let key = field.field_key.as_str();
        let value = match (input.get(key), mode) {
            (Some(provided), _) => {
                if !field.is_active {
                    errors.push(FieldError::new(key, MSG_INACTIVE));
                    continue;
                }
                if field.is_readonly {
                    errors.push(FieldError::new(key, MSG_READONLY));
                    continue;
                }
                match normalize_field_value(field, provided) {
                    Ok(v) => v,
                    Err(message) => {
                        errors.push(FieldError::new(key, message));
                        continue;
                    }
                }
            }
            (None, WriteMode::Create) => {
                if !field.is_active {
                    continue;
                }
                match field.default_value.as_ref() {
                    Some(default) if !default.is_null() => {
                        match normalize_field_value(field, default) {
                            Ok(v) => v,
                            Err(message) => {
                                errors.push(FieldError::new(
                                    key,
                                    format!("預設值設定錯誤：{message}"),
                                ));
                                continue;
                            }
                        }
                    }
                    _ => None,
                }
            }
            // 更新時沒有帶到：保留既有的值
            (None, WriteMode::Update { .. }) => continue,
        };

        match value {
            Some(v) => {
                data.insert(key.to_owned(), v);
            }
            None => {
                // 唯讀欄位使用者無法填寫，不檢查必填
                if field.is_required && !field.is_readonly {
                    errors.push(FieldError::new(key, MSG_REQUIRED));
                }
                data.remove(key);
            }
        }
    }

    for key in input.keys() {
        if !json_fields.iter().any(|f| f.field_key == *key) {
            errors.push(FieldError::new(key, MSG_UNDEFINED));
        }
    }

    if errors.is_empty() {
        Ok(data)
    } else {
        Err(errors)
    }
}

/// 依欄位定義驗證並正規化單一欄位的值，不檢查必填、唯讀與停用。
///
/// 沒有值（`null`、空字串、空陣列）回傳 `Ok(None)`。錯誤為顯示給使用者的說明。
/// 也可用於驗證欄位定義的 `default_value`。
pub fn normalize_field_value(
    field: &FieldDefinition,
    json: &Value,
) -> Result<Option<Value>, String> {
    if matches!(field.data_type, DataType::Relation | DataType::File) && json.as_str() == Some("") {
        return Ok(None);
    }
    let Some(value) = parse_value(field.data_type, json).map_err(|e| e.message().to_owned())?
    else {
        return Ok(None);
    };

    let value = match value {
        FieldValue::Text(s) => {
            if s.is_empty() {
                return Ok(None);
            }
            if field.data_type == DataType::Select {
                check_option(field, &s)?;
            } else {
                check_text(field, &s)?;
            }
            FieldValue::Text(s)
        }
        FieldValue::MultiSelect(items) => {
            if items.is_empty() {
                return Ok(None);
            }
            for item in &items {
                check_option(field, item)?;
            }
            FieldValue::MultiSelect(items)
        }
        FieldValue::Integer(n) => {
            check_range(field, Decimal::from(n))?;
            FieldValue::Integer(n)
        }
        FieldValue::Decimal(mut d) => {
            if let Some(places) = field.decimal_places {
                let places = u32::try_from(places).unwrap_or(0);
                if d.normalize().scale() > places {
                    return Err(format!("小數位數不可超過 {places} 位"));
                }
                // 只會補零，不會失去精度
                d.rescale(places);
            }
            check_range(field, d)?;
            FieldValue::Decimal(d)
        }
        other @ (FieldValue::Date(_)
        | FieldValue::DateTime(_)
        | FieldValue::Boolean(_)
        | FieldValue::Relation(_)
        | FieldValue::File(_)) => other,
    };
    Ok(Some(to_json(&value)))
}

fn check_text(field: &FieldDefinition, s: &str) -> Result<(), String> {
    let Some(validation) = field.validation.as_ref() else {
        return Ok(());
    };
    if let Some(max) = validation.max_length
        && s.chars().count() > max as usize
    {
        return Err(format!("不可超過 {max} 個字元"));
    }
    if field.data_type == DataType::Text
        && let Some(pattern) = validation.pattern.as_deref()
    {
        let re = regex::Regex::new(pattern).map_err(|_| MSG_INVALID_PATTERN.to_owned())?;
        if !re.is_match(s) {
            return Err(MSG_PATTERN.to_owned());
        }
    }
    Ok(())
}

fn check_range(field: &FieldDefinition, n: Decimal) -> Result<(), String> {
    let Some(validation) = field.validation.as_ref() else {
        return Ok(());
    };
    if let Some(min) = validation.min
        && n < min
    {
        return Err(format!("不可小於 {min}"));
    }
    if let Some(max) = validation.max
        && n > max
    {
        return Err(format!("不可大於 {max}"));
    }
    Ok(())
}

fn check_option(field: &FieldDefinition, value: &str) -> Result<(), String> {
    let exists = field
        .options
        .as_deref()
        .is_some_and(|options| options.iter().any(|o| o.value == value));
    if exists {
        Ok(())
    } else {
        Err(format!("「{value}」不是有效的選項"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hammer_shared::schema::{SelectOption, Validation};
    use serde_json::json;
    use std::str::FromStr;
    use time::OffsetDateTime;

    fn field(key: &str, data_type: DataType) -> FieldDefinition {
        FieldDefinition {
            id: hammer_shared::id::new_id(),
            entity_id: uuid::Uuid::nil(),
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

    fn with_validation(mut f: FieldDefinition, v: Validation) -> FieldDefinition {
        f.validation = Some(v);
        f
    }

    fn with_options(mut f: FieldDefinition, values: &[&str]) -> FieldDefinition {
        f.options = Some(
            values
                .iter()
                .map(|v| SelectOption {
                    value: (*v).to_owned(),
                    label: v.to_uppercase(),
                    color: None,
                })
                .collect(),
        );
        f
    }

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn obj(v: Value) -> Map<String, Value> {
        match v {
            Value::Object(m) => m,
            _ => panic!("不是物件"),
        }
    }

    fn create(fields: &[FieldDefinition], input: Value) -> Result<Value, Vec<FieldError>> {
        normalize_record_data(fields, &obj(input), WriteMode::Create).map(Value::Object)
    }

    fn update(
        fields: &[FieldDefinition],
        existing: Value,
        input: Value,
    ) -> Result<Value, Vec<FieldError>> {
        let existing = obj(existing);
        normalize_record_data(
            fields,
            &obj(input),
            WriteMode::Update {
                existing: &existing,
            },
        )
        .map(Value::Object)
    }

    /// 單一欄位的錯誤訊息。
    fn single_error(fields: &[FieldDefinition], input: Value) -> String {
        let errors = create(fields, input).unwrap_err();
        assert_eq!(errors.len(), 1, "{errors:?}");
        errors[0].message.clone()
    }

    // ---- 必填 ----

    #[test]
    fn required_accepts_value() {
        let mut f = field("name", DataType::Text);
        f.is_required = true;
        assert_eq!(
            create(&[f], json!({ "name": "王小明" })).unwrap(),
            json!({ "name": "王小明" })
        );
    }

    #[test]
    fn required_rejects_missing_null_empty() {
        let mut text = field("name", DataType::Text);
        text.is_required = true;
        let mut tags = with_options(field("tags", DataType::MultiSelect), &["a"]);
        tags.is_required = true;
        let fields = [text, tags];

        for input in [
            json!({}),
            json!({ "name": null, "tags": null }),
            json!({ "name": "", "tags": [] }),
        ] {
            let errors = create(&fields, input).unwrap_err();
            assert_eq!(
                errors,
                vec![
                    FieldError::new("name", MSG_REQUIRED),
                    FieldError::new("tags", MSG_REQUIRED),
                ]
            );
        }
    }

    #[test]
    fn required_not_checked_for_inactive_or_readonly() {
        let mut inactive = field("old", DataType::Text);
        inactive.is_required = true;
        inactive.is_active = false;
        let mut readonly = field("sys", DataType::Text);
        readonly.is_required = true;
        readonly.is_readonly = true;
        assert_eq!(create(&[inactive, readonly], json!({})).unwrap(), json!({}));
    }

    #[test]
    fn update_checks_required_only_for_provided_fields() {
        let mut f = field("name", DataType::Text);
        f.is_required = true;
        let other = field("note", DataType::Text);
        let fields = [f, other];
        // 舊資料沒有必填欄位的值，修改其他欄位仍然可以
        assert_eq!(
            update(&fields, json!({}), json!({ "note": "x" })).unwrap(),
            json!({ "note": "x" })
        );
        // 但不可清空必填欄位
        assert_eq!(
            update(&fields, json!({ "name": "A" }), json!({ "name": null })).unwrap_err(),
            vec![FieldError::new("name", MSG_REQUIRED)]
        );
    }

    // ---- 型態檢查 ----

    #[test]
    fn type_check_accepts_each_supported_type() {
        let fields = [
            field("t", DataType::Text),
            field("lt", DataType::LongText),
            field("i", DataType::Integer),
            field("d", DataType::Decimal),
            field("m", DataType::Money),
            field("da", DataType::Date),
            field("dt", DataType::DateTime),
            field("b", DataType::Boolean),
        ];
        let input = json!({
            "t": "a", "lt": "多行\n文字", "i": 42, "d": "1.5", "m": "-10",
            "da": "2026-11-13", "dt": "2026-11-13T08:30:00+08:00", "b": false,
        });
        assert_eq!(create(&fields, input.clone()).unwrap(), input);
    }

    #[test]
    fn type_check_rejects_wrong_json_types() {
        let fields = [
            field("t", DataType::Text),
            field("i", DataType::Integer),
            field("d", DataType::Decimal),
            field("da", DataType::Date),
            field("dt", DataType::DateTime),
            field("b", DataType::Boolean),
            field("ms", DataType::MultiSelect),
        ];
        let input = json!({
            "t": 1, "i": 1.5, "d": 1.5, "da": "2026/11/13", "dt": "2026-11-13",
            "b": "true", "ms": "a",
        });
        let errors = create(&fields, input).unwrap_err();
        let keys: Vec<_> = errors.iter().map(|e| e.field_key.as_str()).collect();
        assert_eq!(keys, ["t", "i", "d", "da", "dt", "b", "ms"]);
        assert_eq!(errors[2].message, "數字必須以文字傳送，例如 \"12.34\"");
    }

    #[test]
    fn unsupported_types_cannot_be_written() {
        let f = field("att", DataType::Formula);
        assert_eq!(
            single_error(&[f], json!({ "att": "x" })),
            "目前不支援此欄位型態"
        );
    }

    #[test]
    fn unsupported_types_preserved_on_update() {
        let f = field("att", DataType::Formula);
        let n = field("n", DataType::Integer);
        assert_eq!(
            update(&[f, n], json!({ "att": "abc" }), json!({ "n": 1 })).unwrap(),
            json!({ "att": "abc", "n": 1 })
        );
    }

    #[test]
    fn file_values_are_normalized() {
        let f = field("att", DataType::File);
        let id = "0190c3a2-0000-7000-8000-000000000000";
        assert_eq!(
            create(
                std::slice::from_ref(&f),
                json!({ "att": "0190C3A2-0000-7000-8000-000000000000" })
            )
            .unwrap(),
            json!({ "att": id })
        );
        assert_eq!(
            create(std::slice::from_ref(&f), json!({ "att": "" })).unwrap(),
            json!({})
        );
        assert_eq!(
            single_error(std::slice::from_ref(&f), json!({ "att": "report.pdf" })),
            "附件 id 格式不正確"
        );
    }

    #[test]
    fn relation_values_are_normalized() {
        let f = field("rel", DataType::Relation);
        let id = "0190c3a2-0000-7000-8000-000000000000";
        assert_eq!(
            create(
                std::slice::from_ref(&f),
                json!({ "rel": "0190C3A2-0000-7000-8000-000000000000" })
            )
            .unwrap(),
            json!({ "rel": id })
        );
        // 空字串、null 視為沒有值
        assert_eq!(
            create(std::slice::from_ref(&f), json!({ "rel": "" })).unwrap(),
            json!({})
        );
        assert_eq!(
            update(
                std::slice::from_ref(&f),
                json!({ "rel": id }),
                json!({ "rel": null })
            )
            .unwrap(),
            json!({})
        );
        assert_eq!(
            single_error(std::slice::from_ref(&f), json!({ "rel": "abc" })),
            "資料 id 格式不正確"
        );
        assert_eq!(
            single_error(std::slice::from_ref(&f), json!({ "rel": 1 })),
            "必須是資料 id（文字）"
        );
        let required = FieldDefinition {
            is_required: true,
            ..f
        };
        assert_eq!(
            single_error(&[required], json!({ "rel": "" })),
            MSG_REQUIRED
        );
    }

    // ---- validation：數值 min / max ----

    #[test]
    fn integer_range() {
        let f = with_validation(
            field("qty", DataType::Integer),
            Validation {
                min: Some(dec("0")),
                max: Some(dec("100")),
                ..Default::default()
            },
        );
        let fields = [f];
        for ok in [0, 50, 100] {
            assert!(create(&fields, json!({ "qty": ok })).is_ok(), "{ok}");
        }
        assert_eq!(single_error(&fields, json!({ "qty": -1 })), "不可小於 0");
        assert_eq!(single_error(&fields, json!({ "qty": 101 })), "不可大於 100");
    }

    #[test]
    fn decimal_range() {
        let f = with_validation(
            field("rate", DataType::Decimal),
            Validation {
                min: Some(dec("0.5")),
                max: Some(dec("1.5")),
                ..Default::default()
            },
        );
        let fields = [f];
        for ok in ["0.5", "1", "1.50"] {
            assert!(create(&fields, json!({ "rate": ok })).is_ok(), "{ok}");
        }
        assert_eq!(
            single_error(&fields, json!({ "rate": "0.49" })),
            "不可小於 0.5"
        );
        assert_eq!(
            single_error(&fields, json!({ "rate": "1.51" })),
            "不可大於 1.5"
        );
    }

    #[test]
    fn money_range() {
        let f = with_validation(
            field("price", DataType::Money),
            Validation {
                min: Some(dec("0")),
                ..Default::default()
            },
        );
        assert!(create(std::slice::from_ref(&f), json!({ "price": "0" })).is_ok());
        assert_eq!(
            single_error(&[f], json!({ "price": "-0.01" })),
            "不可小於 0"
        );
    }

    // ---- validation：文字 max_length / pattern ----

    #[test]
    fn text_max_length_counts_characters() {
        let v = Validation {
            max_length: Some(3),
            ..Default::default()
        };
        let fields = [
            with_validation(field("t", DataType::Text), v.clone()),
            with_validation(field("lt", DataType::LongText), v),
        ];
        // 中文以字元數計算，不是 byte 數
        assert!(create(&fields, json!({ "t": "王小明", "lt": "abc" })).is_ok());
        let errors = create(&fields, json!({ "t": "王小明明", "lt": "abcd" })).unwrap_err();
        assert_eq!(
            errors,
            vec![
                FieldError::new("t", "不可超過 3 個字元"),
                FieldError::new("lt", "不可超過 3 個字元"),
            ]
        );
    }

    #[test]
    fn text_pattern() {
        let f = with_validation(
            field("tax_id", DataType::Text),
            Validation {
                pattern: Some(r"^\d{8}$".to_owned()),
                ..Default::default()
            },
        );
        let fields = [f];
        assert!(create(&fields, json!({ "tax_id": "12345678" })).is_ok());
        assert_eq!(
            single_error(&fields, json!({ "tax_id": "1234567" })),
            MSG_PATTERN
        );
    }

    #[test]
    fn invalid_pattern_reports_setting_error() {
        let f = with_validation(
            field("t", DataType::Text),
            Validation {
                pattern: Some("(".to_owned()),
                ..Default::default()
            },
        );
        assert_eq!(single_error(&[f], json!({ "t": "a" })), MSG_INVALID_PATTERN);
    }

    // ---- select / multi_select ----

    #[test]
    fn select_value_must_be_in_options() {
        let fields = [with_options(
            field("status", DataType::Select),
            &["open", "closed"],
        )];
        assert_eq!(
            create(&fields, json!({ "status": "open" })).unwrap(),
            json!({ "status": "open" })
        );
        assert_eq!(
            single_error(&fields, json!({ "status": "OPEN" })),
            "「OPEN」不是有效的選項"
        );
    }

    #[test]
    fn select_without_options_rejects_any_value() {
        let fields = [field("status", DataType::Select)];
        assert_eq!(
            single_error(&fields, json!({ "status": "x" })),
            "「x」不是有效的選項"
        );
    }

    #[test]
    fn multi_select_values_must_be_in_options() {
        let fields = [with_options(
            field("tags", DataType::MultiSelect),
            &["a", "b", "c"],
        )];
        assert_eq!(
            create(&fields, json!({ "tags": ["c", "a"] })).unwrap(),
            json!({ "tags": ["c", "a"] })
        );
        assert_eq!(
            single_error(&fields, json!({ "tags": ["a", "z"] })),
            "「z」不是有效的選項"
        );
    }

    #[test]
    fn multi_select_rejects_duplicates() {
        let fields = [with_options(
            field("tags", DataType::MultiSelect),
            &["a", "b"],
        )];
        assert_eq!(
            single_error(&fields, json!({ "tags": ["a", "a"] })),
            "選項不可重複"
        );
    }

    // ---- decimal_places ----

    #[test]
    fn decimal_places_pads_without_rounding() {
        let mut d = field("amount", DataType::Decimal);
        d.decimal_places = Some(2);
        let mut m = field("price", DataType::Money);
        m.decimal_places = Some(4);
        let fields = [d, m];
        assert_eq!(
            create(&fields, json!({ "amount": "12.3", "price": "5" })).unwrap(),
            json!({ "amount": "12.30", "price": "5.0000" })
        );
        // 尾端的 0 不算超過
        assert_eq!(
            create(&fields, json!({ "amount": "1.500", "price": "1" })).unwrap(),
            json!({ "amount": "1.50", "price": "1.0000" })
        );
    }

    #[test]
    fn decimal_places_exceeded_is_rejected() {
        let mut d = field("amount", DataType::Decimal);
        d.decimal_places = Some(2);
        let mut m = field("price", DataType::Money);
        m.decimal_places = Some(0);
        let errors = create(&[d, m], json!({ "amount": "12.345", "price": "1.5" })).unwrap_err();
        assert_eq!(
            errors,
            vec![
                FieldError::new("amount", "小數位數不可超過 2 位"),
                FieldError::new("price", "小數位數不可超過 0 位"),
            ]
        );
    }

    // ---- 唯讀 ----

    #[test]
    fn readonly_cannot_be_written() {
        let mut f = field("code", DataType::Text);
        f.is_readonly = true;
        let fields = [f];
        assert_eq!(single_error(&fields, json!({ "code": "x" })), MSG_READONLY);
        assert_eq!(
            update(&fields, json!({ "code": "A" }), json!({ "code": "B" })).unwrap_err(),
            vec![FieldError::new("code", MSG_READONLY)]
        );
        // 沒有帶到時保留既有的值
        assert_eq!(
            update(&fields, json!({ "code": "A" }), json!({})).unwrap(),
            json!({ "code": "A" })
        );
    }

    #[test]
    fn readonly_gets_default_on_create() {
        let mut f = field("status", DataType::Text);
        f.is_readonly = true;
        f.default_value = Some(json!("draft"));
        assert_eq!(
            create(&[f], json!({})).unwrap(),
            json!({ "status": "draft" })
        );
    }

    // ---- 未定義、已停用 ----

    #[test]
    fn undefined_field_key_is_rejected() {
        let fields = [field("name", DataType::Text)];
        assert_eq!(single_error(&fields, json!({ "nmae": "x" })), MSG_UNDEFINED);
    }

    #[test]
    fn column_storage_field_is_treated_as_undefined() {
        let mut f = field("code", DataType::Text);
        f.storage = FieldStorage::Column;
        f.is_system = true;
        assert_eq!(single_error(&[f], json!({ "code": "x" })), MSG_UNDEFINED);
    }

    #[test]
    fn inactive_field_cannot_be_written() {
        let mut f = field("old", DataType::Text);
        f.is_active = false;
        let fields = [f];
        assert_eq!(single_error(&fields, json!({ "old": "x" })), MSG_INACTIVE);
        assert_eq!(
            update(&fields, json!({ "old": "A" }), json!({ "old": null })).unwrap_err(),
            vec![FieldError::new("old", MSG_INACTIVE)]
        );
    }

    #[test]
    fn inactive_field_value_preserved_on_update() {
        let mut old = field("old", DataType::Text);
        old.is_active = false;
        let name = field("name", DataType::Text);
        let fields = [old, name];
        assert_eq!(
            update(
                &fields,
                json!({ "old": "保留", "name": "A" }),
                json!({ "name": "B" })
            )
            .unwrap(),
            json!({ "old": "保留", "name": "B" })
        );
    }

    #[test]
    fn inactive_field_default_not_applied() {
        let mut f = field("old", DataType::Text);
        f.is_active = false;
        f.default_value = Some(json!("x"));
        assert_eq!(create(&[f], json!({})).unwrap(), json!({}));
    }

    // ---- 預設值 ----

    #[test]
    fn default_value_applied_on_create_when_missing() {
        let mut status = with_options(field("status", DataType::Select), &["open", "closed"]);
        status.default_value = Some(json!("open"));
        let mut qty = field("qty", DataType::Decimal);
        qty.decimal_places = Some(2);
        qty.default_value = Some(json!("1"));
        let fields = [status, qty];
        assert_eq!(
            create(&fields, json!({})).unwrap(),
            json!({ "status": "open", "qty": "1.00" })
        );
        // 有帶值時使用帶的值；明確帶 null 代表不要值
        assert_eq!(
            create(&fields, json!({ "status": "closed", "qty": null })).unwrap(),
            json!({ "status": "closed" })
        );
    }

    #[test]
    fn default_value_satisfies_required() {
        let mut f = field("name", DataType::Text);
        f.is_required = true;
        f.default_value = Some(json!("未命名"));
        assert_eq!(
            create(&[f], json!({})).unwrap(),
            json!({ "name": "未命名" })
        );
    }

    #[test]
    fn default_value_not_applied_on_update() {
        let mut f = field("status", DataType::Text);
        f.default_value = Some(json!("open"));
        assert_eq!(update(&[f], json!({}), json!({})).unwrap(), json!({}));
    }

    #[test]
    fn invalid_default_value_is_reported() {
        let mut f = with_options(field("status", DataType::Select), &["open"]);
        f.default_value = Some(json!("gone"));
        assert_eq!(
            single_error(&[f], json!({})),
            "預設值設定錯誤：「gone」不是有效的選項"
        );
    }

    // ---- 更新的合併 ----

    #[test]
    fn update_merges_and_null_clears() {
        let fields = [field("a", DataType::Text), field("b", DataType::Integer)];
        assert_eq!(
            update(
                &fields,
                json!({ "a": "x", "b": 1 }),
                json!({ "a": null, "b": 2 })
            )
            .unwrap(),
            json!({ "b": 2 })
        );
    }

    // ---- 多個錯誤 ----

    #[test]
    fn reports_all_field_errors_at_once() {
        let mut name = field("name", DataType::Text);
        name.is_required = true;
        let qty = with_validation(
            field("qty", DataType::Integer),
            Validation {
                min: Some(dec("1")),
                ..Default::default()
            },
        );
        let status = with_options(field("status", DataType::Select), &["open"]);
        let mut code = field("code", DataType::Text);
        code.is_readonly = true;
        let fields = [name, qty, status, code];

        let errors = create(
            &fields,
            json!({ "qty": 0, "status": "x", "code": "A", "zzz": 1, "aaa": 2 }),
        )
        .unwrap_err();
        assert_eq!(
            errors,
            vec![
                FieldError::new("name", MSG_REQUIRED),
                FieldError::new("qty", "不可小於 1"),
                FieldError::new("status", "「x」不是有效的選項"),
                FieldError::new("code", MSG_READONLY),
                FieldError::new("aaa", MSG_UNDEFINED),
                FieldError::new("zzz", MSG_UNDEFINED),
            ]
        );
    }

    // ---- 單一值 ----

    #[test]
    fn normalize_field_value_treats_empty_as_none() {
        let f = field("t", DataType::Text);
        assert_eq!(normalize_field_value(&f, &json!("")), Ok(None));
        assert_eq!(normalize_field_value(&f, &Value::Null), Ok(None));
        assert_eq!(normalize_field_value(&f, &json!(" ")), Ok(Some(json!(" "))));
    }
}

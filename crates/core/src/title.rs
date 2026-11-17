//! 資料標題（docs/architecture.md §8.5 `entity_definitions.title_template`）。
//!
//! 關聯欄位、選擇器、操作紀錄用來表示「這筆資料叫什麼」的文字。
//!
//! # 範本
//!
//! 範本由文字與代碼組成，代碼寫在 `{}` 內，例如 `{record_no} {name}`：
//!
//! - `{record_no}`：單號（[`RECORD_NO`]）；沒有單號時為空字串
//! - `{field_key}`：本資料類別的欄位
//! - `{relation_field.field_key}`：關聯引用，關聯欄位所指資料的欄位（或 `record_no`），只能一層
//!
//! `{}` 以外的文字原樣保留，但不可有單獨的 `{`、`}` 或控制字元；長度上限 [`MAX_TEMPLATE_LEN`]
//! 個字元。
//!
//! 寫入資料類別前以 [`normalize_template`] 檢查：只能引用 `record_no` 或本資料類別已定義的欄位
//! （含已停用）；關聯引用的前半段必須是已定義、型態為 `relation` 的欄位，後半段必須是目標資料類別
//! 已定義的欄位（含已停用）或 `record_no`。
//!
//! # 套用（[`TitleRenderer`]）
//!
//! 欄位值依型態格式化，沒有值時為空字串：
//!
//! | 型態 | 標題中的文字 |
//! |---|---|
//! | `text`、`long_text` | 原樣；換行等控制字元換成空白 |
//! | `integer`、`decimal`、`money` | 存值原樣（不加千分位；小數位數在寫入時已補齊） |
//! | `date` | `YYYY-MM-DD` |
//! | `datetime` | 台北時間 `YYYY-MM-DD HH:mm`（D13） |
//! | `boolean` | `是` / `否` |
//! | `select` | 選項名稱；選項已移除時為選項值 |
//! | `multi_select` | 選項名稱，以 `、` 連接 |
//! | `formula` | 依 `formula_result_type` 套用上列規則 |
//! | `relation`、`file` | 空字串（關聯目標的欄位請用關聯引用） |
//!
//! 引用已停用或不存在的欄位、存值格式不正確時視為沒有值。結果去除前後空白。
//!
//! 關聯引用讀取目標資料**目前**的值（含已軟刪除的資料），依目標欄位的型態套用上表；目標欄位是
//! `relation` 或 `file` 時為空字串（不再往下一層）。關聯欄位沒有值、找不到目標資料、關聯欄位或
//! 目標欄位已停用、關聯欄位已變更為其他型態時為空字串。目標資料由呼叫端批次讀取後以
//! [`RelatedRecords`] 傳入（[`TitleRenderer::target_entities`]、[`TitleRenderer::related_ids`]）。
//!
//! # 沒有設定範本時
//!
//! 依序使用第一個有值的：
//!
//! 1. 單號（`record_no`）
//! 2. 排序最前面、有效（啟用）的 `text` 欄位的值
//! 3. id 的前 8 碼
//!
//! 資料庫內的範本不合法時（例如加入檢查之前存入的），也使用以上預設規則。

use std::collections::{BTreeSet, HashMap, HashSet};

use hammer_shared::Id;
use hammer_shared::record::RECORD_NO;
use hammer_shared::schema::{DataType, FieldDefinition, FieldValue, parse_value, validate_key};
use hammer_shared::time::TAIPEI_OFFSET;
use serde_json::{Map, Value};
use time::macros::format_description;

/// 標題範本的長度上限（字元數）。
pub const MAX_TEMPLATE_LEN: usize = 200;

/// 沒有範本、單號與文字欄位時，標題使用的 id 字元數。
pub const ID_PREFIX_LEN: usize = 8;

/// 範本的組成單位。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Literal(String),
    RecordNo,
    Field(String),
    /// `{relation.field}`；`field` 可能是 [`RECORD_NO`]。
    Relation {
        relation: String,
        field: String,
    },
}

/// 解析後的標題範本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TitleTemplate {
    parts: Vec<Part>,
}

/// 標題範本不合法。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateError {
    /// 超過 [`MAX_TEMPLATE_LEN`] 個字元。
    TooLong,
    /// 有 `{` 沒有對應的 `}`。
    UnclosedBrace,
    /// 有 `}` 沒有對應的 `{`。
    UnmatchedCloseBrace,
    /// 含有控制字元（例如換行）。
    ControlChar,
    /// `{}` 內不是合法的欄位代碼。
    InvalidCode(String),
    /// 兩層以上的關聯引用（`{customer.region.name}`），不支援。
    NestedReference(String),
    /// 引用本資料類別沒有的欄位。
    UnknownField(String),
    /// 關聯引用的前半段不是 `relation` 欄位。
    NotRelation(String),
    /// 關聯引用的後半段不是目標資料類別的欄位（或找不到目標資料類別）。
    UnknownTargetField { relation: String, field: String },
}

impl TemplateError {
    /// 顯示給使用者的訊息。
    pub fn message(&self) -> String {
        match self {
            Self::TooLong => format!("標題範本不可超過 {MAX_TEMPLATE_LEN} 個字元"),
            Self::UnclosedBrace => "標題範本的「{」沒有對應的「}」".to_owned(),
            Self::UnmatchedCloseBrace => "標題範本的「}」沒有對應的「{」".to_owned(),
            Self::ControlChar => "標題範本不可包含換行等控制字元".to_owned(),
            Self::InvalidCode(code) => format!(
                "「{{{code}}}」不是合法的欄位代碼；請寫成 {{欄位代碼}}、{{record_no}} 或 {{關聯欄位.欄位代碼}}"
            ),
            Self::NestedReference(code) => {
                format!("「{{{code}}}」引用超過一層關聯；標題範本只能引用一層關聯資料的欄位")
            }
            Self::UnknownField(key) => format!("此資料類別沒有欄位「{key}」"),
            Self::NotRelation(key) => {
                format!("欄位「{key}」不是關聯欄位，不能引用「{key}.欄位代碼」")
            }
            Self::UnknownTargetField { relation, field } => {
                format!("關聯欄位「{relation}」所指的資料類別沒有欄位「{field}」")
            }
        }
    }
}

impl std::fmt::Display for TemplateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for TemplateError {}

impl TitleTemplate {
    /// 解析範本，只檢查語法，不檢查欄位是否存在。不會去除前後空白（見 [`normalize_template`]）。
    pub fn parse(template: &str) -> Result<Self, TemplateError> {
        if template.chars().count() > MAX_TEMPLATE_LEN {
            return Err(TemplateError::TooLong);
        }
        if template.chars().any(char::is_control) {
            return Err(TemplateError::ControlChar);
        }

        let mut parts = Vec::new();
        let mut literal = String::new();
        let mut rest = template;
        while let Some(c) = rest.chars().next() {
            match c {
                '{' => {
                    let end = rest.find('}').ok_or(TemplateError::UnclosedBrace)?;
                    let code = &rest[1..end];
                    if code.contains('{') {
                        return Err(TemplateError::UnclosedBrace);
                    }
                    if !literal.is_empty() {
                        parts.push(Part::Literal(std::mem::take(&mut literal)));
                    }
                    parts.push(parse_code(code)?);
                    rest = &rest[end + 1..];
                }
                '}' => return Err(TemplateError::UnmatchedCloseBrace),
                _ => {
                    literal.push(c);
                    rest = &rest[c.len_utf8()..];
                }
            }
        }
        if !literal.is_empty() {
            parts.push(Part::Literal(literal));
        }
        Ok(Self { parts })
    }

    /// 範本引用的本資料類別欄位代碼（不含 `record_no`；關聯引用取前半段），依出現順序、不重複。
    pub fn field_keys(&self) -> Vec<&str> {
        let mut seen = HashSet::new();
        self.parts
            .iter()
            .filter_map(|p| match p {
                Part::Field(key) => Some(key.as_str()),
                Part::Relation { relation, .. } => Some(relation.as_str()),
                _ => None,
            })
            .filter(|key| seen.insert(*key))
            .collect()
    }

    /// 檢查範本時需要欄位定義的目標資料類別：關聯引用的前半段是 `fields` 中（含已停用）型態為
    /// `relation` 的欄位時，它的 `relation_entity_id`。排序、不重複。
    pub fn relation_targets(&self, fields: &[FieldDefinition]) -> Vec<Id> {
        self.relation_fields(fields)
            .filter_map(|f| f.relation_entity_id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    fn relation_fields<'f>(
        &'f self,
        fields: &'f [FieldDefinition],
    ) -> impl Iterator<Item = &'f FieldDefinition> {
        self.parts.iter().filter_map(move |p| match p {
            Part::Relation { relation, .. } => fields
                .iter()
                .find(|f| f.field_key == *relation && f.data_type == DataType::Relation),
            _ => None,
        })
    }
}

fn parse_code(code: &str) -> Result<Part, TemplateError> {
    if code == RECORD_NO {
        return Ok(Part::RecordNo);
    }
    if let Some((relation, field)) = code.split_once('.') {
        if field.contains('.') {
            return Err(TemplateError::NestedReference(code.to_owned()));
        }
        let invalid = || TemplateError::InvalidCode(code.to_owned());
        validate_key(relation).map_err(|_| invalid())?;
        if field != RECORD_NO {
            validate_key(field).map_err(|_| invalid())?;
        }
        return Ok(Part::Relation {
            relation: relation.to_owned(),
            field: field.to_owned(),
        });
    }
    validate_key(code).map_err(|_| TemplateError::InvalidCode(code.to_owned()))?;
    Ok(Part::Field(code.to_owned()))
}

/// 關聯目標資料類別的欄位定義（含已停用），鍵為資料類別 id。
pub type TargetFields = HashMap<Id, Vec<FieldDefinition>>;

/// 寫入資料類別前檢查並整理範本：去除前後空白，空白視為沒有範本（`None`）。
///
/// `fields` 為資料類別目前的所有欄位定義（含已停用）；範本只能引用其中的欄位或 `record_no`。
/// `targets` 為關聯引用的目標資料類別的欄位定義（[`TitleTemplate::relation_targets`] 列出的
/// 資料類別；找不到的資料類別不放入）。
pub fn normalize_template(
    template: Option<&str>,
    fields: &[FieldDefinition],
    targets: &TargetFields,
) -> Result<Option<String>, TemplateError> {
    let Some(template) = template.map(str::trim).filter(|t| !t.is_empty()) else {
        return Ok(None);
    };
    let parsed = TitleTemplate::parse(template)?;
    let find = |key: &str| {
        fields
            .iter()
            .find(|f| f.field_key == key)
            .ok_or_else(|| TemplateError::UnknownField(key.to_owned()))
    };
    for part in &parsed.parts {
        match part {
            Part::Field(key) => {
                find(key)?;
            }
            Part::Relation { relation, field } => {
                let def = find(relation)?;
                if def.data_type != DataType::Relation {
                    return Err(TemplateError::NotRelation(relation.clone()));
                }
                // 目標資料類別不存在時也無法引用 record_no
                let found = def
                    .relation_entity_id
                    .and_then(|id| targets.get(&id))
                    .is_some_and(|t| field == RECORD_NO || t.iter().any(|f| f.field_key == *field));
                if !found {
                    return Err(TemplateError::UnknownTargetField {
                        relation: relation.clone(),
                        field: field.clone(),
                    });
                }
            }
            Part::Literal(_) | Part::RecordNo => {}
        }
    }
    Ok(Some(template.to_owned()))
}

/// 關聯引用讀取的一筆目標資料。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RelatedRecord {
    pub record_no: Option<String>,
    pub data: Map<String, Value>,
}

/// 關聯引用讀取的目標資料，鍵為 `(目標資料類別 id, 資料 id)`。
pub type RelatedRecords = HashMap<(Id, Id), RelatedRecord>;

/// 依資料類別的範本與欄位定義產生資料的標題。
#[derive(Debug, Clone)]
pub struct TitleRenderer<'a> {
    template: Option<TitleTemplate>,
    fields: &'a [FieldDefinition],
    /// 沒有範本時使用的文字欄位：排序最前面、有效的 `text` 欄位。
    default_field: Option<&'a FieldDefinition>,
    /// 關聯引用的目標資料類別的欄位定義。
    targets: TargetFields,
}

impl<'a> TitleRenderer<'a> {
    /// `template` 為資料類別的 `title_template`；`fields` 為所有欄位定義（含已停用）。
    /// 範本不合法時視為沒有範本。
    pub fn new(template: Option<&str>, fields: &'a [FieldDefinition]) -> Self {
        let template = template
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .and_then(|t| TitleTemplate::parse(t).ok());
        let default_field = fields
            .iter()
            .filter(|f| f.is_active && f.data_type == DataType::Text)
            .min_by_key(|f| f.sort_order);
        Self {
            template,
            fields,
            default_field,
            targets: TargetFields::new(),
        }
    }

    /// 產生標題需要讀取的目標資料類別：範本的關聯引用中，有效、型態為 `relation` 的欄位的目標。
    /// 排序、不重複。呼叫端讀取這些資料類別的欄位定義後以 [`Self::set_target_fields`] 設定。
    pub fn target_entities(&self) -> Vec<Id> {
        let Some(template) = &self.template else {
            return Vec::new();
        };
        template
            .relation_fields(self.fields)
            .filter(|f| f.is_active)
            .filter_map(|f| f.relation_entity_id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// 設定目標資料類別 `entity_id` 的欄位定義（含已停用）。沒有設定的資料類別，關聯引用為空字串。
    pub fn set_target_fields(&mut self, entity_id: Id, fields: Vec<FieldDefinition>) {
        self.targets.insert(entity_id, fields);
    }

    /// 產生 `data` 的標題需要讀取的目標資料 `(目標資料類別 id, 資料 id)`，不重複。
    /// 只列出已設定欄位定義（[`Self::set_target_fields`]）的目標資料類別。
    pub fn related_ids(&self, data: &Map<String, Value>) -> Vec<(Id, Id)> {
        let Some(template) = &self.template else {
            return Vec::new();
        };
        let mut out: Vec<(Id, Id)> = template
            .relation_fields(self.fields)
            .filter(|f| f.is_active)
            .filter_map(|f| {
                let entity_id = f.relation_entity_id?;
                if !self.targets.contains_key(&entity_id) {
                    return None;
                }
                Some((entity_id, relation_id(data.get(&f.field_key)?)?))
            })
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// 產生一筆資料的標題。`related` 為關聯引用的目標資料（沒有關聯引用時可傳空的）。
    pub fn render(
        &self,
        id: Id,
        record_no: Option<&str>,
        data: &Map<String, Value>,
        related: &RelatedRecords,
    ) -> String {
        let Some(template) = &self.template else {
            return self.default_title(id, record_no, data);
        };
        let mut title = String::new();
        for part in &template.parts {
            match part {
                Part::Literal(text) => title.push_str(text),
                Part::RecordNo => title.push_str(record_no.unwrap_or_default()),
                Part::Field(key) => {
                    if let Some(field) = self.fields.iter().find(|f| f.field_key == *key) {
                        title.push_str(&field_text(field, data));
                    }
                }
                Part::Relation { relation, field } => {
                    title.push_str(&self.relation_text(relation, field, data, related));
                }
            }
        }
        title.trim().to_owned()
    }

    /// 關聯引用 `{relation.field}` 的文字。
    fn relation_text(
        &self,
        relation: &str,
        field: &str,
        data: &Map<String, Value>,
        related: &RelatedRecords,
    ) -> String {
        let Some(def) = self.fields.iter().find(|f| f.field_key == relation) else {
            return String::new();
        };
        // 關聯欄位已停用、已變更為其他型態 → 空字串
        if !def.is_active || def.data_type != DataType::Relation {
            return String::new();
        }
        let Some(entity_id) = def.relation_entity_id else {
            return String::new();
        };
        let Some(target_id) = data.get(relation).and_then(relation_id) else {
            return String::new();
        };
        let Some(record) = related.get(&(entity_id, target_id)) else {
            return String::new();
        };
        if field == RECORD_NO {
            return sanitize(record.record_no.as_deref().unwrap_or_default());
        }
        self.targets
            .get(&entity_id)
            .and_then(|fields| fields.iter().find(|f| f.field_key == field))
            .map(|f| field_text(f, &record.data))
            .unwrap_or_default()
    }

    fn default_title(&self, id: Id, record_no: Option<&str>, data: &Map<String, Value>) -> String {
        if let Some(no) = record_no.map(str::trim).filter(|s| !s.is_empty()) {
            return no.to_owned();
        }
        if let Some(field) = self.default_field {
            let text = field_text(field, data);
            let text = text.trim();
            if !text.is_empty() {
                return text.to_owned();
            }
        }
        id.to_string().chars().take(ID_PREFIX_LEN).collect()
    }
}

fn relation_id(value: &Value) -> Option<Id> {
    match parse_value(DataType::Relation, value) {
        Ok(Some(FieldValue::Relation(id))) => Some(id),
        _ => None,
    }
}

/// 控制字元換成空白。
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// 欄位值在標題中的文字。已停用、沒有值或格式不正確時為空字串。
///
/// `formula` 欄位的存值格式與它的 `formula_result_type` 相同（§7.3），依結果型態格式化。
fn field_text(field: &FieldDefinition, data: &Map<String, Value>) -> String {
    let value_type = match field.data_type {
        DataType::Formula => match field.formula_result_type {
            Some(t) => t,
            None => return String::new(),
        },
        t => t,
    };
    if !field.is_active || !value_type.is_value_supported() {
        return String::new();
    }
    let Some(json) = data.get(&field.field_key) else {
        return String::new();
    };
    let Ok(Some(value)) = parse_value(value_type, json) else {
        return String::new();
    };
    match value {
        FieldValue::Text(s) if field.data_type == DataType::Select => option_label(field, &s),
        FieldValue::Text(s) => sanitize(&s),
        FieldValue::Integer(n) => n.to_string(),
        FieldValue::Decimal(d) => d.to_string(),
        FieldValue::Date(d) => d
            .format(format_description!("[year]-[month]-[day]"))
            .unwrap_or_default(),
        FieldValue::DateTime(dt) => dt
            .to_offset(TAIPEI_OFFSET)
            .format(format_description!("[year]-[month]-[day] [hour]:[minute]"))
            .unwrap_or_default(),
        FieldValue::Boolean(b) => if b { "是" } else { "否" }.to_owned(),
        FieldValue::MultiSelect(items) => items
            .iter()
            .map(|v| option_label(field, v))
            .collect::<Vec<_>>()
            .join("、"),
        // 關聯目標的欄位請用關聯引用 `{relation.field}`，不會再往下一層
        FieldValue::Relation(_) | FieldValue::File(_) => String::new(),
    }
}

fn option_label(field: &FieldDefinition, value: &str) -> String {
    field
        .options
        .as_ref()
        .and_then(|opts| opts.iter().find(|o| o.value == value))
        .map_or_else(|| value.to_owned(), |o| o.label.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hammer_shared::new_id;
    use hammer_shared::schema::{FieldStorage, SelectOption};
    use serde_json::json;
    use time::OffsetDateTime;

    fn field(key: &str, data_type: DataType, sort_order: i32) -> FieldDefinition {
        FieldDefinition {
            id: new_id(),
            entity_id: new_id(),
            field_key: key.into(),
            label: key.into(),
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
            sort_order,
            is_active: true,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            version: 1,
        }
    }

    fn options(values: &[(&str, &str)]) -> Option<Vec<SelectOption>> {
        Some(
            values
                .iter()
                .map(|(v, l)| SelectOption {
                    value: (*v).into(),
                    label: (*l).into(),
                    color: None,
                })
                .collect(),
        )
    }

    fn none() -> RelatedRecords {
        RelatedRecords::new()
    }

    fn normalize_template_(
        template: Option<&str>,
        fields: &[FieldDefinition],
    ) -> Result<Option<String>, TemplateError> {
        normalize_template(template, fields, &TargetFields::new())
    }

    fn data(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    fn render(template: &str, fields: &[FieldDefinition], v: Value) -> String {
        TitleRenderer::new(Some(template), fields).render(new_id(), None, &data(v), &none())
    }

    // ---- 解析與檢查 ----

    #[test]
    fn parses_literals_record_no_and_fields() {
        let t = TitleTemplate::parse("RO-{record_no} {name}（{qty}）{name}").unwrap();
        assert_eq!(
            t.parts,
            vec![
                Part::Literal("RO-".into()),
                Part::RecordNo,
                Part::Literal(" ".into()),
                Part::Field("name".into()),
                Part::Literal("（".into()),
                Part::Field("qty".into()),
                Part::Literal("）".into()),
                Part::Field("name".into()),
            ]
        );
        assert_eq!(t.field_keys(), vec!["name", "qty"]);
    }

    #[test]
    fn rejects_invalid_syntax() {
        let long = "a".repeat(MAX_TEMPLATE_LEN + 1);
        let cases = [
            (long.as_str(), TemplateError::TooLong),
            ("{name", TemplateError::UnclosedBrace),
            ("{na{me}", TemplateError::UnclosedBrace),
            ("name}", TemplateError::UnmatchedCloseBrace),
            ("{name}\n", TemplateError::ControlChar),
            ("{}", TemplateError::InvalidCode(String::new())),
            ("{Name}", TemplateError::InvalidCode("Name".into())),
            ("{ name }", TemplateError::InvalidCode(" name ".into())),
            (
                "{customer.region.name}",
                TemplateError::NestedReference("customer.region.name".into()),
            ),
            ("{.name}", TemplateError::InvalidCode(".name".into())),
            (
                "{customer.}",
                TemplateError::InvalidCode("customer.".into()),
            ),
            (
                "{customer.Name}",
                TemplateError::InvalidCode("customer.Name".into()),
            ),
        ];
        for (template, expected) in cases {
            assert_eq!(TitleTemplate::parse(template), Err(expected), "{template}");
        }
        assert!(TitleTemplate::parse(&"a".repeat(MAX_TEMPLATE_LEN)).is_ok());
    }

    #[test]
    fn normalize_checks_fields_exist() {
        let mut inactive = field("old", DataType::Text, 1);
        inactive.is_active = false;
        let fields = [field("name", DataType::Text, 0), inactive];

        assert_eq!(normalize_template_(None, &fields), Ok(None));
        assert_eq!(normalize_template_(Some("  "), &fields), Ok(None));
        assert_eq!(
            normalize_template_(Some(" {record_no} {name} "), &fields),
            Ok(Some("{record_no} {name}".into()))
        );
        // 已停用的欄位仍是已定義的欄位
        assert_eq!(
            normalize_template_(Some("{old}"), &fields),
            Ok(Some("{old}".into()))
        );
        // 沒有欄位時只能用 record_no
        assert_eq!(
            normalize_template_(Some("{record_no}"), &[]),
            Ok(Some("{record_no}".into()))
        );
        assert_eq!(
            normalize_template_(Some("{name} {missing}"), &fields),
            Err(TemplateError::UnknownField("missing".into()))
        );
        assert_eq!(
            normalize_template_(Some("{name"), &fields),
            Err(TemplateError::UnclosedBrace)
        );
    }

    // ---- 各型態的格式 ----

    #[test]
    fn formats_values_by_type() {
        let mut status = field("status", DataType::Select, 0);
        status.options = options(&[("open", "處理中"), ("done", "已完成")]);
        let mut tags = field("tags", DataType::MultiSelect, 0);
        tags.options = options(&[("a", "急件"), ("b", "保固")]);
        let mut price = field("price", DataType::Money, 0);
        price.decimal_places = Some(2);
        let fields = [
            field("name", DataType::Text, 0),
            field("note", DataType::LongText, 0),
            field("qty", DataType::Integer, 0),
            field("rate", DataType::Decimal, 0),
            price,
            field("day", DataType::Date, 0),
            field("at", DataType::DateTime, 0),
            field("ok", DataType::Boolean, 0),
            status,
            tags,
        ];
        let cases = [
            ("{name}", json!({ "name": "壓縮機" }), "壓縮機"),
            (
                "{note}",
                json!({ "note": "第一行\n第二行" }),
                "第一行 第二行",
            ),
            ("{qty}", json!({ "qty": 1234567 }), "1234567"),
            ("{qty}", json!({ "qty": -5 }), "-5"),
            ("{rate}", json!({ "rate": "0.1250" }), "0.1250"),
            ("{price}", json!({ "price": "1200.50" }), "1200.50"),
            ("{day}", json!({ "day": "2026-11-05" }), "2026-11-05"),
            // UTC 16:30 已是台北隔天 00:30
            (
                "{at}",
                json!({ "at": "2026-11-05T16:30:00Z" }),
                "2026-11-06 00:30",
            ),
            (
                "{at}",
                json!({ "at": "2026-11-05T09:05:59+08:00" }),
                "2026-11-05 09:05",
            ),
            ("{ok}", json!({ "ok": true }), "是"),
            ("{ok}", json!({ "ok": false }), "否"),
            ("{status}", json!({ "status": "done" }), "已完成"),
            // 選項已移除時顯示選項值
            ("{status}", json!({ "status": "gone" }), "gone"),
            ("{tags}", json!({ "tags": ["b", "a"] }), "保固、急件"),
        ];
        for (template, v, expected) in cases {
            assert_eq!(
                render(template, &fields, v.clone()),
                expected,
                "{template} {v}"
            );
        }
    }

    #[test]
    fn combines_literals_and_record_no() {
        let fields = [field("name", DataType::Text, 0)];
        let r = TitleRenderer::new(Some("{record_no} {name}"), &fields);
        let d = data(json!({ "name": "壓縮機" }));
        assert_eq!(
            r.render(new_id(), Some("RO-001"), &d, &none()),
            "RO-001 壓縮機"
        );
        // 沒有單號時為空字串，結果去除前後空白
        assert_eq!(r.render(new_id(), None, &d, &none()), "壓縮機");
    }

    #[test]
    fn empty_values_are_empty_strings() {
        let fields = [
            field("name", DataType::Text, 0),
            field("qty", DataType::Integer, 0),
            field("ok", DataType::Boolean, 0),
        ];
        assert_eq!(
            render("[{name}][{qty}][{ok}]", &fields, json!({})),
            "[][][]"
        );
        assert_eq!(
            render(
                "[{name}][{qty}]",
                &fields,
                json!({ "name": null, "qty": null })
            ),
            "[][]"
        );
        // 格式不正確的存值視為沒有值
        assert_eq!(render("[{qty}]", &fields, json!({ "qty": "abc" })), "[]");
        // 範本結果全空時就是空字串，不改用預設規則
        assert_eq!(render("{name}", &fields, json!({})), "");
    }

    #[test]
    fn inactive_and_missing_fields_are_empty() {
        let mut old = field("old", DataType::Text, 0);
        old.is_active = false;
        let fields = [field("name", DataType::Text, 0), old];
        let v = json!({ "name": "A", "old": "停用欄位的值", "missing": "x" });
        assert_eq!(render("{name}/{old}/{missing}", &fields, v), "A//");
    }

    #[test]
    fn unsupported_types_are_empty() {
        let fields = [field("customer", DataType::Relation, 0)];
        assert_eq!(
            render(
                "[{customer}]",
                &fields,
                json!({ "customer": new_id().to_string() })
            ),
            "[]"
        );
    }

    fn formula(key: &str, result: Option<DataType>) -> FieldDefinition {
        let mut f = field(key, DataType::Formula, 0);
        f.formula = Some("1".into());
        f.formula_result_type = result;
        f
    }

    #[test]
    fn formula_fields_format_by_result_type() {
        let mut inactive = formula("old", Some(DataType::Text));
        inactive.is_active = false;
        let fields = [
            formula("f_text", Some(DataType::Text)),
            formula("f_long", Some(DataType::LongText)),
            formula("f_int", Some(DataType::Integer)),
            formula("f_dec", Some(DataType::Decimal)),
            formula("f_money", Some(DataType::Money)),
            formula("f_day", Some(DataType::Date)),
            formula("f_at", Some(DataType::DateTime)),
            formula("f_ok", Some(DataType::Boolean)),
            formula("f_none", None),
            inactive,
        ];
        let cases = [
            ("{f_text}", json!({ "f_text": "A-01" }), "A-01"),
            ("{f_long}", json!({ "f_long": "a\nb" }), "a b"),
            ("{f_int}", json!({ "f_int": 42 }), "42"),
            ("{f_dec}", json!({ "f_dec": "0.1250" }), "0.1250"),
            ("{f_money}", json!({ "f_money": "1200.50" }), "1200.50"),
            ("{f_day}", json!({ "f_day": "2026-11-05" }), "2026-11-05"),
            (
                "{f_at}",
                json!({ "f_at": "2026-11-05T16:30:00Z" }),
                "2026-11-06 00:30",
            ),
            ("{f_ok}", json!({ "f_ok": true }), "是"),
            // 計算失敗（null）、沒有值、存值格式不符結果型態 → 空字串
            ("[{f_int}]", json!({ "f_int": null }), "[]"),
            ("[{f_int}]", json!({}), "[]"),
            ("[{f_int}]", json!({ "f_int": "abc" }), "[]"),
            // 沒有結果型態（資料庫中的定義不完整）、已停用 → 空字串
            ("[{f_none}]", json!({ "f_none": "x" }), "[]"),
            ("[{old}]", json!({ "old": "x" }), "[]"),
        ];
        for (template, v, expected) in cases {
            assert_eq!(
                render(template, &fields, v.clone()),
                expected,
                "{template} {v}"
            );
        }
    }

    #[test]
    fn formula_fields_are_not_default_title() {
        // 預設規則只使用 `text` 欄位，不使用結果為文字的公式欄位
        let fields = [formula("code", Some(DataType::Text))];
        let id = new_id();
        let title = TitleRenderer::new(None, &fields).render(
            id,
            None,
            &data(json!({ "code": "A-01" })),
            &none(),
        );
        assert_eq!(title, id.to_string()[..ID_PREFIX_LEN]);
    }

    // ---- 預設規則 ----

    #[test]
    fn default_prefers_record_no() {
        let fields = [field("name", DataType::Text, 0)];
        let r = TitleRenderer::new(None, &fields);
        let d = data(json!({ "name": "壓縮機" }));
        assert_eq!(r.render(new_id(), Some("RO-001"), &d, &none()), "RO-001");
    }

    #[test]
    fn default_uses_first_active_text_field() {
        let mut first_inactive = field("code", DataType::Text, 0);
        first_inactive.is_active = false;
        let fields = [
            field("qty", DataType::Integer, 0),
            field("note", DataType::LongText, 1),
            field("model", DataType::Text, 3),
            field("name", DataType::Text, 2),
            first_inactive,
        ];
        let r = TitleRenderer::new(None, &fields);
        let d = data(json!({ "qty": 1, "note": "n", "code": "C", "name": "壓縮機", "model": "M" }));
        assert_eq!(r.render(new_id(), None, &d, &none()), "壓縮機");
    }

    #[test]
    fn default_falls_back_to_id_prefix() {
        let id = new_id();
        let prefix: String = id.to_string().chars().take(ID_PREFIX_LEN).collect();

        // 沒有文字欄位
        let fields = [field("qty", DataType::Integer, 0)];
        let r = TitleRenderer::new(None, &fields);
        assert_eq!(
            r.render(id, None, &data(json!({ "qty": 1 })), &none()),
            prefix
        );

        // 有文字欄位但沒有值
        let fields = [field("name", DataType::Text, 0)];
        let r = TitleRenderer::new(None, &fields);
        assert_eq!(r.render(id, None, &data(json!({})), &none()), prefix);
        assert_eq!(
            r.render(id, Some(""), &data(json!({ "name": " " })), &none()),
            prefix
        );
        assert_eq!(prefix.len(), ID_PREFIX_LEN);
    }

    #[test]
    fn blank_or_invalid_stored_template_uses_default() {
        let fields = [field("name", DataType::Text, 0)];
        let d = data(json!({ "name": "壓縮機" }));
        for template in ["", "  ", "{name", "{customer.region.name}"] {
            let r = TitleRenderer::new(Some(template), &fields);
            assert_eq!(
                r.render(new_id(), None, &d, &none()),
                "壓縮機",
                "{template:?}"
            );
        }
    }

    // ---- 關聯引用 ----

    /// 目標資料類別 `customer` 的欄位，與指向它的關聯欄位 `customer`。
    struct RelationFixture {
        customer_entity: Id,
        fields: Vec<FieldDefinition>,
        targets: TargetFields,
    }

    fn relation_fixture() -> RelationFixture {
        let customer_entity = new_id();
        let mut rel = field("customer", DataType::Relation, 1);
        rel.relation_entity_id = Some(customer_entity);
        let mut grade = field("grade", DataType::Select, 2);
        grade.options = options(&[("a", "A 級")]);
        let mut old = field("old", DataType::Text, 9);
        old.is_active = false;
        let mut credit = field("credit", DataType::Money, 3);
        credit.decimal_places = Some(2);
        let target_fields = vec![
            field("short_name", DataType::Text, 0),
            grade,
            credit,
            field("since", DataType::Date, 4),
            field("vip", DataType::Boolean, 5),
            field("parent", DataType::Relation, 6),
            field("logo", DataType::File, 7),
            old,
        ];
        RelationFixture {
            customer_entity,
            fields: vec![field("name", DataType::Text, 0), rel],
            targets: TargetFields::from([(customer_entity, target_fields)]),
        }
    }

    impl RelationFixture {
        fn renderer(&self, template: &str) -> TitleRenderer<'_> {
            let mut r = TitleRenderer::new(Some(template), &self.fields);
            for id in r.target_entities() {
                r.set_target_fields(id, self.targets[&id].clone());
            }
            r
        }
    }

    #[test]
    fn parses_relation_references() {
        let t =
            TitleTemplate::parse("{record_no} {customer.short_name}{customer.record_no}").unwrap();
        assert_eq!(
            t.parts,
            vec![
                Part::RecordNo,
                Part::Literal(" ".into()),
                Part::Relation {
                    relation: "customer".into(),
                    field: "short_name".into(),
                },
                Part::Relation {
                    relation: "customer".into(),
                    field: RECORD_NO.into(),
                },
            ]
        );
        assert_eq!(t.field_keys(), vec!["customer"]);
    }

    #[test]
    fn normalize_checks_relation_references() {
        let fx = relation_fixture();
        let ok = |t: &str| normalize_template(Some(t), &fx.fields, &fx.targets);
        assert_eq!(
            ok("{record_no} {customer.short_name}"),
            Ok(Some("{record_no} {customer.short_name}".into()))
        );
        // 目標的單號、已停用的目標欄位、目標的關聯欄位都是已定義的
        for t in [
            "{customer.record_no}",
            "{customer.old}",
            "{customer.parent}",
        ] {
            assert_eq!(ok(t), Ok(Some(t.into())), "{t}");
        }
        // 前半段不存在、不是關聯欄位
        assert_eq!(
            ok("{vendor.name}"),
            Err(TemplateError::UnknownField("vendor".into()))
        );
        assert_eq!(
            ok("{name.short_name}"),
            Err(TemplateError::NotRelation("name".into()))
        );
        // 後半段不是目標資料類別的欄位（本資料類別的欄位也不算）
        assert_eq!(
            ok("{customer.name}"),
            Err(TemplateError::UnknownTargetField {
                relation: "customer".into(),
                field: "name".into(),
            })
        );
        // 兩層
        assert_eq!(
            ok("{customer.parent.short_name}"),
            Err(TemplateError::NestedReference(
                "customer.parent.short_name".into()
            ))
        );
        // 找不到目標資料類別（沒有放進 targets）時，連 record_no 也不可引用
        let no_targets = TargetFields::new();
        for t in ["{customer.short_name}", "{customer.record_no}"] {
            assert!(
                matches!(
                    normalize_template(Some(t), &fx.fields, &no_targets),
                    Err(TemplateError::UnknownTargetField { .. })
                ),
                "{t}"
            );
        }
        // 已停用的關聯欄位仍可被引用
        let mut fields = fx.fields.clone();
        fields[1].is_active = false;
        assert!(normalize_template(Some("{customer.short_name}"), &fields, &fx.targets).is_ok());
        for e in [
            TemplateError::NotRelation("x".into()),
            TemplateError::NestedReference("a.b.c".into()),
            TemplateError::UnknownTargetField {
                relation: "a".into(),
                field: "b".into(),
            },
        ] {
            assert!(!e.message().is_empty());
        }
    }

    #[test]
    fn relation_targets_and_related_ids() {
        let fx = relation_fixture();
        let t = TitleTemplate::parse("{customer.short_name} {name} {customer.vip}").unwrap();
        assert_eq!(t.relation_targets(&fx.fields), vec![fx.customer_entity]);
        // 已停用的關聯欄位：檢查時要讀取目標，產生標題時不需要
        let mut fields = fx.fields.clone();
        fields[1].is_active = false;
        assert_eq!(t.relation_targets(&fields), vec![fx.customer_entity]);
        let r = TitleRenderer::new(Some("{customer.short_name}"), &fields);
        assert!(r.target_entities().is_empty());

        let r = fx.renderer("{customer.short_name} {customer.vip}");
        assert_eq!(r.target_entities(), vec![fx.customer_entity]);
        let target = new_id();
        let d = data(json!({ "customer": target.to_string() }));
        assert_eq!(r.related_ids(&d), vec![(fx.customer_entity, target)]);
        // 沒有值、值不是 UUID → 不需讀取
        assert!(r.related_ids(&data(json!({}))).is_empty());
        assert!(
            r.related_ids(&data(json!({ "customer": "abc" })))
                .is_empty()
        );
        // 沒有設定目標欄位定義 → 不需讀取
        let r = TitleRenderer::new(Some("{customer.short_name}"), &fx.fields);
        assert!(r.related_ids(&d).is_empty());
        // 沒有範本、沒有關聯引用
        assert!(
            TitleRenderer::new(None, &fx.fields)
                .target_entities()
                .is_empty()
        );
        assert!(fx.renderer("{name}").target_entities().is_empty());
    }

    #[test]
    fn renders_relation_references_by_target_type() {
        let fx = relation_fixture();
        let target = new_id();
        let related = RelatedRecords::from([(
            (fx.customer_entity, target),
            RelatedRecord {
                record_no: Some("C-001".into()),
                data: data(json!({
                    "short_name": "大同\n電器",
                    "grade": "a",
                    "credit": "50000.00",
                    "since": "2020-01-31",
                    "vip": true,
                    "parent": new_id().to_string(),
                    "logo": new_id().to_string(),
                    "old": "停用",
                })),
            },
        )]);
        let d = data(json!({ "name": "冷氣", "customer": target.to_string() }));
        let cases = [
            ("{name} {customer.short_name}", "冷氣 大同 電器"),
            ("{customer.record_no}", "C-001"),
            ("{customer.grade}", "A 級"),
            ("{customer.credit}", "50000.00"),
            ("{customer.since}", "2020-01-31"),
            ("{customer.vip}", "是"),
            // 目標欄位是 relation、file 或已停用 → 空字串
            (
                "[{customer.parent}][{customer.logo}][{customer.old}]",
                "[][][]",
            ),
        ];
        for (template, expected) in cases {
            let r = fx.renderer(template);
            assert_eq!(
                r.render(new_id(), None, &d, &related),
                expected,
                "{template}"
            );
        }
    }

    #[test]
    fn relation_references_are_empty_when_unavailable() {
        let fx = relation_fixture();
        let target = new_id();
        let related = RelatedRecords::from([(
            (fx.customer_entity, target),
            RelatedRecord {
                record_no: None,
                data: data(json!({ "short_name": "大同" })),
            },
        )]);
        let template = "RO {customer.short_name}[{customer.record_no}]";
        let with = data(json!({ "customer": target.to_string() }));
        let r = fx.renderer(template);
        assert_eq!(r.render(new_id(), None, &with, &related), "RO 大同[]");
        // 關聯欄位沒有值
        assert_eq!(
            r.render(new_id(), None, &data(json!({})), &related),
            "RO []"
        );
        // 找不到目標資料
        let other = data(json!({ "customer": new_id().to_string() }));
        assert_eq!(r.render(new_id(), None, &other, &related), "RO []");
        // 沒有讀取目標資料
        assert_eq!(r.render(new_id(), None, &with, &none()), "RO []");

        // 關聯欄位已停用
        let mut fields = fx.fields.clone();
        fields[1].is_active = false;
        let mut r = TitleRenderer::new(Some(template), &fields);
        r.set_target_fields(fx.customer_entity, fx.targets[&fx.customer_entity].clone());
        assert_eq!(r.render(new_id(), None, &with, &related), "RO []");

        // 關聯欄位已變更為其他型態（例如文字，存值仍是原本的 id）
        let mut fields = fx.fields.clone();
        fields[1].data_type = DataType::Text;
        let mut r = TitleRenderer::new(Some(template), &fields);
        r.set_target_fields(fx.customer_entity, fx.targets[&fx.customer_entity].clone());
        assert_eq!(r.render(new_id(), None, &with, &related), "RO []");

        // 目標欄位已不存在於目標資料類別
        let mut r = TitleRenderer::new(Some("[{customer.gone}]"), &fx.fields);
        r.set_target_fields(fx.customer_entity, fx.targets[&fx.customer_entity].clone());
        assert_eq!(r.render(new_id(), None, &with, &related), "[]");
    }
}

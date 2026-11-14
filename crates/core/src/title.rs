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
//!
//! `{}` 以外的文字原樣保留，但不可有單獨的 `{`、`}` 或控制字元；長度上限 [`MAX_TEMPLATE_LEN`]
//! 個字元。引用關聯欄位的欄位（例如 `{customer.name}`）目前不支援。
//!
//! 寫入資料類別前以 [`normalize_template`] 檢查：只能引用 `record_no` 或本資料類別已定義的欄位
//! （含已停用）。
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
//! | `relation`、`file`、`formula` | 目前為空字串 |
//!
//! 引用已停用或不存在的欄位、存值格式不正確時視為沒有值。結果去除前後空白。
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

use std::collections::HashSet;

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
    /// 引用關聯欄位的欄位（`{customer.name}`），目前不支援。
    NestedReference(String),
    /// 引用本資料類別沒有的欄位。
    UnknownField(String),
}

impl TemplateError {
    /// 顯示給使用者的訊息。
    pub fn message(&self) -> String {
        match self {
            Self::TooLong => format!("標題範本不可超過 {MAX_TEMPLATE_LEN} 個字元"),
            Self::UnclosedBrace => "標題範本的「{」沒有對應的「}」".to_owned(),
            Self::UnmatchedCloseBrace => "標題範本的「}」沒有對應的「{」".to_owned(),
            Self::ControlChar => "標題範本不可包含換行等控制字元".to_owned(),
            Self::InvalidCode(code) => {
                format!("「{{{code}}}」不是合法的欄位代碼；請寫成 {{欄位代碼}} 或 {{record_no}}")
            }
            Self::NestedReference(code) => {
                format!("目前不支援引用關聯資料的欄位「{{{code}}}」")
            }
            Self::UnknownField(key) => format!("此資料類別沒有欄位「{key}」"),
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

    /// 範本引用的欄位代碼（不含 `record_no`），依出現順序、不重複。
    pub fn field_keys(&self) -> Vec<&str> {
        let mut seen = HashSet::new();
        self.parts
            .iter()
            .filter_map(|p| match p {
                Part::Field(key) => Some(key.as_str()),
                _ => None,
            })
            .filter(|key| seen.insert(*key))
            .collect()
    }
}

fn parse_code(code: &str) -> Result<Part, TemplateError> {
    if code == RECORD_NO {
        return Ok(Part::RecordNo);
    }
    if code.contains('.') {
        return Err(TemplateError::NestedReference(code.to_owned()));
    }
    validate_key(code).map_err(|_| TemplateError::InvalidCode(code.to_owned()))?;
    Ok(Part::Field(code.to_owned()))
}

/// 寫入資料類別前檢查並整理範本：去除前後空白，空白視為沒有範本（`None`）。
///
/// `fields` 為資料類別目前的所有欄位定義（含已停用）；範本只能引用其中的欄位或 `record_no`。
pub fn normalize_template(
    template: Option<&str>,
    fields: &[FieldDefinition],
) -> Result<Option<String>, TemplateError> {
    let Some(template) = template.map(str::trim).filter(|t| !t.is_empty()) else {
        return Ok(None);
    };
    let parsed = TitleTemplate::parse(template)?;
    if let Some(missing) = parsed
        .field_keys()
        .into_iter()
        .find(|key| !fields.iter().any(|f| f.field_key == *key))
    {
        return Err(TemplateError::UnknownField(missing.to_owned()));
    }
    Ok(Some(template.to_owned()))
}

/// 依資料類別的範本與欄位定義產生資料的標題。
#[derive(Debug, Clone)]
pub struct TitleRenderer<'a> {
    template: Option<TitleTemplate>,
    fields: &'a [FieldDefinition],
    /// 沒有範本時使用的文字欄位：排序最前面、有效的 `text` 欄位。
    default_field: Option<&'a FieldDefinition>,
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
        }
    }

    /// 產生一筆資料的標題。
    pub fn render(&self, id: Id, record_no: Option<&str>, data: &Map<String, Value>) -> String {
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
            }
        }
        title.trim().to_owned()
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

/// 欄位值在標題中的文字。已停用、沒有值或格式不正確時為空字串。
fn field_text(field: &FieldDefinition, data: &Map<String, Value>) -> String {
    if !field.is_active || !field.data_type.is_value_supported() {
        return String::new();
    }
    let Some(json) = data.get(&field.field_key) else {
        return String::new();
    };
    let Ok(Some(value)) = parse_value(field.data_type, json) else {
        return String::new();
    };
    match value {
        FieldValue::Text(s) if field.data_type == DataType::Select => option_label(field, &s),
        FieldValue::Text(s) => s
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect(),
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
        // 引用關聯資料的欄位（例如 `{customer.name}`）等 M3 一起評估
        FieldValue::Relation(_) => String::new(),
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

    fn data(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    fn render(template: &str, fields: &[FieldDefinition], v: Value) -> String {
        TitleRenderer::new(Some(template), fields).render(new_id(), None, &data(v))
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
                "{customer.name}",
                TemplateError::NestedReference("customer.name".into()),
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

        assert_eq!(normalize_template(None, &fields), Ok(None));
        assert_eq!(normalize_template(Some("  "), &fields), Ok(None));
        assert_eq!(
            normalize_template(Some(" {record_no} {name} "), &fields),
            Ok(Some("{record_no} {name}".into()))
        );
        // 已停用的欄位仍是已定義的欄位
        assert_eq!(
            normalize_template(Some("{old}"), &fields),
            Ok(Some("{old}".into()))
        );
        // 沒有欄位時只能用 record_no
        assert_eq!(
            normalize_template(Some("{record_no}"), &[]),
            Ok(Some("{record_no}".into()))
        );
        assert_eq!(
            normalize_template(Some("{name} {missing}"), &fields),
            Err(TemplateError::UnknownField("missing".into()))
        );
        assert_eq!(
            normalize_template(Some("{name"), &fields),
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
        assert_eq!(r.render(new_id(), Some("RO-001"), &d), "RO-001 壓縮機");
        // 沒有單號時為空字串，結果去除前後空白
        assert_eq!(r.render(new_id(), None, &d), "壓縮機");
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

    // ---- 預設規則 ----

    #[test]
    fn default_prefers_record_no() {
        let fields = [field("name", DataType::Text, 0)];
        let r = TitleRenderer::new(None, &fields);
        let d = data(json!({ "name": "壓縮機" }));
        assert_eq!(r.render(new_id(), Some("RO-001"), &d), "RO-001");
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
        assert_eq!(r.render(new_id(), None, &d), "壓縮機");
    }

    #[test]
    fn default_falls_back_to_id_prefix() {
        let id = new_id();
        let prefix: String = id.to_string().chars().take(ID_PREFIX_LEN).collect();

        // 沒有文字欄位
        let fields = [field("qty", DataType::Integer, 0)];
        let r = TitleRenderer::new(None, &fields);
        assert_eq!(r.render(id, None, &data(json!({ "qty": 1 }))), prefix);

        // 有文字欄位但沒有值
        let fields = [field("name", DataType::Text, 0)];
        let r = TitleRenderer::new(None, &fields);
        assert_eq!(r.render(id, None, &data(json!({}))), prefix);
        assert_eq!(
            r.render(id, Some(""), &data(json!({ "name": " " }))),
            prefix
        );
        assert_eq!(prefix.len(), ID_PREFIX_LEN);
    }

    #[test]
    fn blank_or_invalid_stored_template_uses_default() {
        let fields = [field("name", DataType::Text, 0)];
        let d = data(json!({ "name": "壓縮機" }));
        for template in ["", "  ", "{name", "{customer.name}"] {
            let r = TitleRenderer::new(Some(template), &fields);
            assert_eq!(r.render(new_id(), None, &d), "壓縮機", "{template:?}");
        }
    }
}

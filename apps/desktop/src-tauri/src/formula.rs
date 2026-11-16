//! 桌面端直接使用 `crates/formula` 的功能（`docs/architecture.md` §3.3）：
//!
//! - [`check_syntax`]：公式的即時語法檢查。只檢查語法；引用的欄位是否存在、型態、循環引用需要
//!   其他欄位定義，由伺服器在儲存時檢查（§7.6）。
//! - [`preview`]：表單中公式欄位的即時預覽。與伺服器存檔時使用同一套計算
//!   （[`hammer_formula::record`]），存檔時仍以伺服器計算的結果為準。

use std::collections::{BTreeMap, HashMap};

use hammer_formula::record::{FormulaErrors, Formulas, Targets};
use hammer_shared::Id;
use hammer_shared::schema::{DataType, FieldDefinition};
use hammer_shared::time::Date;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use specta::Type;
use specta_typescript::Unknown;

/// 公式的語法錯誤。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct FormulaSyntaxError {
    /// 錯誤訊息（繁體中文，不含位置）。
    pub message: String,
    /// 錯誤在公式中的位置：**字元**位移（Unicode 字元，不是 UTF-16），從 0 開始。
    pub start: u32,
    /// 錯誤範圍的結尾（不含）；與 `start` 相同時代表該位置缺少內容（例如公式不完整）。
    pub end: u32,
}

/// 檢查公式語法，正確時回傳 `None`。
pub fn check_syntax(formula: &str) -> Option<FormulaSyntaxError> {
    let err = hammer_formula::parse(formula).err()?;
    // 公式長度有上限（`MAX_FORMULA_LEN`），位移不會超過 u32
    let to_u32 = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    Some(FormulaSyntaxError {
        message: err.message(),
        start: to_u32(err.span.start),
        end: to_u32(err.span.end),
    })
}

/// 公式預覽的輸入：欄位定義與本筆資料的快照（JSON 存值，§7.3）。
#[derive(Debug, Clone, Deserialize, Type)]
pub struct FormulaPreviewRequest {
    /// 主表的欄位定義（可含已停用的欄位，計算時略過）。
    pub fields: Vec<FieldDefinition>,
    /// 有效的明細子表與其欄位定義。
    #[serde(default)]
    pub details: Vec<DetailSchema>,
    /// 主表的資料，鍵為 `field_key`。
    #[specta(type = HashMap<String, Unknown>)]
    pub data: Map<String, Value>,
    /// 明細，鍵為明細子表代碼，依順序。
    #[serde(default)]
    #[specta(type = HashMap<String, Vec<HashMap<String, Unknown>>>)]
    pub lines: BTreeMap<String, Vec<Map<String, Value>>>,
    /// 關聯引用所指資料的快照。沒有列出的資料類別讀不到目標欄位，引用它的公式為錯誤；
    /// 表單不提供（關聯引用的欄位顯示上一次存檔的值，見 [`FormulaPreview::relation_dependent`]）。
    #[serde(default)]
    pub targets: Vec<TargetSnapshot>,
}

/// 一個明細子表的欄位定義。
#[derive(Debug, Clone, Deserialize, Type)]
pub struct DetailSchema {
    /// 明細子表代碼。
    pub key: String,
    pub fields: Vec<FieldDefinition>,
}

/// 關聯目標資料類別的快照。
#[derive(Debug, Clone, Deserialize, Type)]
pub struct TargetSnapshot {
    pub entity_id: Id,
    pub fields: Vec<FieldDefinition>,
    #[serde(default)]
    pub records: Vec<TargetRecord>,
}

/// 關聯目標的一筆資料。
#[derive(Debug, Clone, Deserialize, Type)]
pub struct TargetRecord {
    pub id: Id,
    #[specta(type = HashMap<String, Unknown>)]
    pub data: Map<String, Value>,
}

/// 公式預覽的結果。
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
pub struct FormulaPreview {
    /// 主表的公式欄位。
    pub master: RowPreview,
    /// 明細的公式欄位，鍵為明細子表代碼，順序與輸入相同。
    pub lines: BTreeMap<String, Vec<RowPreview>>,
    /// 直接或間接使用關聯引用的公式欄位。預覽沒有關聯資料的最新值，表單顯示上一次存檔的值，
    /// 並標示存檔後更新。
    pub relation_dependent: Vec<PreviewField>,
}

/// 一列資料（主表或一列明細）的預覽。
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
pub struct RowPreview {
    /// 有效公式欄位的計算結果（JSON 存值，§7.3），沒有值或計算失敗時為 `null`。
    #[specta(type = HashMap<String, Unknown>)]
    pub values: Map<String, Value>,
    /// 計算失敗的欄位 → 錯誤原因（繁體中文）。
    pub errors: FormulaErrors,
}

/// 預覽中的一個公式欄位。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct PreviewField {
    /// 明細子表代碼；主表的欄位為 `null`。
    pub detail: Option<String>,
    pub field_key: String,
}

/// 計算公式欄位的預覽：與伺服器存檔時相同（`hammer_core::records`），先算每一列明細，再算主表，
/// 主表的子表彙總讀取明細的計算結果。`today` 為公式的 `TODAY()`。
///
/// 輸入的公式欄位值一律被計算結果取代（與伺服器相同）。
pub fn preview(req: &FormulaPreviewRequest, today: Date) -> FormulaPreview {
    let details: Vec<(&str, &[FieldDefinition])> = req
        .details
        .iter()
        .map(|d| (d.key.as_str(), d.fields.as_slice()))
        .collect();
    let formulas = Formulas::new(&req.fields, &details);
    let mut targets = Targets::default();
    for t in &req.targets {
        targets.insert_entity(t.entity_id, &t.fields);
        for r in &t.records {
            targets.insert_record(t.entity_id, r.id, r.data.clone());
        }
    }

    let mut computed: BTreeMap<&str, Vec<ComputedRow>> = BTreeMap::new();
    for (key, rows) in &req.lines {
        let rows = rows
            .iter()
            .map(|row| {
                let mut data = row.clone();
                let errors = formulas.compute_line(key, &mut data, &targets, today);
                (data, errors)
            })
            .collect();
        computed.insert(key.as_str(), rows);
    }
    let rows: BTreeMap<&str, Vec<&Map<String, Value>>> = computed
        .iter()
        .map(|(key, rows)| (*key, rows.iter().map(|(data, _)| data).collect()))
        .collect();
    let mut data = req.data.clone();
    let errors = formulas.compute_master(&mut data, &rows, &targets, today);

    let line_previews = computed
        .into_iter()
        .map(|(key, rows)| {
            let fields = details
                .iter()
                .find(|(k, _)| *k == key)
                .map_or(&[][..], |(_, f)| f);
            let previews = rows
                .into_iter()
                .map(|(data, errors)| row_preview(fields, &data, errors))
                .collect();
            (key.to_owned(), previews)
        })
        .collect();
    FormulaPreview {
        master: row_preview(&req.fields, &data, errors),
        lines: line_previews,
        relation_dependent: formulas
            .relation_dependent()
            .map(|f| PreviewField {
                detail: f.detail.clone(),
                field_key: f.field.clone(),
            })
            .collect(),
    }
}

/// 計算後的一列：`data` 與失敗的欄位。
type ComputedRow = (Map<String, Value>, FormulaErrors);

/// 取出有效公式欄位的值。
fn row_preview(
    fields: &[FieldDefinition],
    data: &Map<String, Value>,
    errors: FormulaErrors,
) -> RowPreview {
    RowPreview {
        values: fields
            .iter()
            .filter(|f| f.is_active && f.data_type == DataType::Formula)
            .map(|f| {
                let value = data.get(&f.field_key).cloned().unwrap_or(Value::Null);
                (f.field_key.clone(), value)
            })
            .collect(),
        errors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_formula_has_no_error() {
        assert_eq!(check_syntax("{qty} * {unit_price}"), None);
        assert_eq!(check_syntax("SUM(lines.amount) + {customer.rate}"), None);
    }

    #[test]
    fn reports_message_and_char_span() {
        let err = check_syntax("{qty} * ").unwrap();
        assert_eq!(err.message, "公式不完整");
        assert_eq!((err.start, err.end), (8, 8));

        // 位置以字元計算（中文字串不以 byte 計）
        let err = check_syntax("\"數量\" & @").unwrap();
        assert_eq!(err.message, "無法辨識的字元「@」");
        assert_eq!((err.start, err.end), (7, 8));
    }

    #[test]
    fn empty_formula_is_an_error() {
        let err = check_syntax("   ").unwrap();
        assert_eq!(err.message, "公式不可為空白");
    }

    /// 與伺服器（`hammer_core::recalculate` 的測試）共用的測試資料：預覽結果必須等於 `expected`。
    const SHARED_CASE: &str =
        include_str!("../../../../crates/formula/testdata/record_preview.json");

    fn shared_case() -> (FormulaPreviewRequest, Date, Value) {
        let case: Value = serde_json::from_str(SHARED_CASE).unwrap();
        let req: FormulaPreviewRequest = serde_json::from_value(case.clone()).unwrap();
        let today = Date::parse(
            case["today"].as_str().unwrap(),
            time::macros::format_description!("[year]-[month]-[day]"),
        )
        .unwrap();
        (req, today, case["expected"].clone())
    }

    #[test]
    fn preview_matches_server_calculation() {
        let (req, today, expected) = shared_case();
        let preview = preview(&req, today);
        assert_eq!(
            serde_json::to_value(&preview.master).unwrap(),
            expected["master"]
        );
        assert_eq!(
            serde_json::to_value(&preview.lines).unwrap(),
            expected["lines"]
        );
    }

    #[test]
    fn lists_relation_dependent_fields() {
        let (req, today, _) = shared_case();
        let got: Vec<String> = preview(&req, today)
            .relation_dependent
            .into_iter()
            .map(|f| match f.detail {
                Some(d) => format!("{d}.{}", f.field_key),
                None => f.field_key,
            })
            .collect();
        assert_eq!(got, ["discount_amount", "label", "total"]);
    }

    #[test]
    fn without_targets_relation_formulas_are_errors_and_others_still_compute() {
        let (mut req, today, expected) = shared_case();
        req.targets.clear();
        let preview = preview(&req, today);
        assert_eq!(
            preview.master.values["subtotal"],
            expected["master"]["values"]["subtotal"]
        );
        assert!(preview.master.errors.contains_key("discount_amount"));
        assert!(preview.master.errors.contains_key("label"));
    }

    #[test]
    fn lines_of_unknown_detail_have_no_values() {
        let (mut req, today, _) = shared_case();
        req.lines
            .insert("other".into(), vec![serde_json::Map::new()]);
        let preview = preview(&req, today);
        assert_eq!(preview.lines["other"].len(), 1);
        assert!(preview.lines["other"][0].values.is_empty());
    }
}

//! 存檔時計算公式欄位（docs/architecture.md §7.6、§8.5 `custom_records`）。
//!
//! 新增與修改資料時（[`crate::records`]），在寫入的 transaction 內：
//!
//! 1. [`Formulas::new`]：解析資料類別（主表與有效的明細子表）所有**有效**的公式欄位，依相依圖決定
//!    計算順序（[`hammer_formula::calculation_order`]）
//! 2. [`Formulas::targets`]、[`load_targets`]：讀取公式的關聯引用 `{relation.field}` 所指資料**目前**的值（含已軟刪除的資料）
//!    與目標資料類別的欄位型態
//! 3. [`Formulas::compute_line`]：依序計算每一列明細的公式
//! 4. [`Formulas::compute_master`]：計算主表的公式，子表彙總讀取步驟 3 的結果
//!
//! 結果寫入 `data`（沒有值的欄位從 `data` 移除，與一般欄位相同）；失敗的欄位在 `data` 中沒有值，
//! 原因記在 [`FormulaErrors`]（存入 `custom_records.formula_errors`），不中斷存檔。
//!
//! 「今天」（`TODAY()`）為存檔時間在 Asia/Taipei 的日期（D13，[`today_of`]）。
//!
//! 公式引用的欄位型態依目前的欄位定義：只有**有效**的欄位可以引用，`formula` 欄位以它的
//! `formula_result_type` 讀取。已停用的公式欄位不計算，既有的值保留。
//!
//! 資料庫中的公式有循環引用或無法解析（定義時已檢查，只有資料不一致時才會發生）時，相關欄位的
//! 結果為空值並記錄原因，一樣不中斷存檔。

use std::collections::{BTreeMap, BTreeSet, HashMap};

use hammer_db::record as record_db;
use hammer_db::schema as schema_db;
use hammer_db::sqlx::PgConnection;
use hammer_formula::{
    DetailColumn, Environment, Expr, FieldData, FieldRef, FormulaField, calculation_order, compute,
    dependencies, parse,
};
use hammer_shared::Id;
use hammer_shared::schema::{DataType, FieldDefinition, FieldValue, parse_value};
use hammer_shared::time::{Date, TAIPEI_OFFSET};
use serde_json::{Map, Value};
use time::OffsetDateTime;

use crate::CoreError;
use crate::schema::field_from_row;

/// 一筆資料的公式計算錯誤：欄位代碼 → 錯誤原因（繁體中文）。
pub type FormulaErrors = BTreeMap<String, String>;

/// 公式欄位沒有結果型態（資料庫內容不一致）時的錯誤訊息。
pub const MSG_NO_RESULT_TYPE: &str = "公式欄位沒有設定結果型態，請聯絡管理者";

/// 存檔時間在 Asia/Taipei 的日期，即公式的 `TODAY()`（D13）。
pub fn today_of(now: OffsetDateTime) -> Date {
    now.to_offset(TAIPEI_OFFSET).date()
}

/// [`FormulaErrors`] 轉成 `custom_records.formula_errors` 的值：沒有錯誤時為 `None`（資料庫 `NULL`）。
pub fn errors_to_json(errors: &FormulaErrors) -> Option<Value> {
    if errors.is_empty() {
        return None;
    }
    Some(Value::Object(
        errors
            .iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect(),
    ))
}

/// `custom_records.formula_errors` 轉成 [`FormulaErrors`]；不是物件或值不是字串的項目略過。
pub fn errors_from_json(json: Option<&Value>) -> FormulaErrors {
    json.and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default()
}

/// 一個公式欄位（已解析）。
#[derive(Debug, Clone)]
struct Compiled {
    field_key: String,
    /// 解析結果；無法計算時為錯誤原因。
    expr: Result<Expr, String>,
    result_type: Option<DataType>,
    decimal_places: Option<i16>,
}

/// 一張表（主表或一個明細子表）可被公式讀取的欄位。
#[derive(Debug, Clone, Default)]
struct TableSchema {
    /// 有效欄位的代碼 → 公式中的型態（`formula` 為結果型態）。
    types: HashMap<String, DataType>,
    /// 有效關聯欄位的代碼 → 目標資料類別。
    relations: HashMap<String, Id>,
    /// 依計算順序的公式欄位。
    formulas: Vec<Compiled>,
    /// 公式引用的關聯欄位（[`load_targets`] 要讀取的目標）。
    used_relations: BTreeSet<String>,
}

impl TableSchema {
    fn new(fields: &[FieldDefinition]) -> Self {
        let active = || fields.iter().filter(|f| f.is_active);
        Self {
            types: active()
                .filter_map(|f| Some((f.field_key.clone(), value_type(f)?)))
                .collect(),
            relations: active()
                .filter(|f| f.data_type == DataType::Relation)
                .filter_map(|f| Some((f.field_key.clone(), f.relation_entity_id?)))
                .collect(),
            formulas: Vec::new(),
            used_relations: BTreeSet::new(),
        }
    }
}

/// 欄位在公式與查詢中的值型態：`formula` 為它的結果型態（沒有設定時為 `None`），其他為欄位型態。
pub fn value_type(field: &FieldDefinition) -> Option<DataType> {
    match field.data_type {
        DataType::Formula => field.formula_result_type,
        t => Some(t),
    }
}

/// 一個資料類別（主表與有效的明細子表）的公式欄位與計算順序。
#[derive(Debug, Clone, Default)]
pub struct Formulas {
    master: TableSchema,
    /// 有效的明細子表代碼 → 欄位。
    details: BTreeMap<String, TableSchema>,
}

impl Formulas {
    /// `master_fields` 為主表的欄位定義、`details` 為**有效**明細子表的代碼與欄位定義（皆可含已停用的
    /// 欄位，計算時略過）。
    pub fn new(master_fields: &[FieldDefinition], details: &[(&str, &[FieldDefinition])]) -> Self {
        let mut out = Self {
            master: TableSchema::new(master_fields),
            details: details
                .iter()
                .map(|(key, fields)| ((*key).to_owned(), TableSchema::new(fields)))
                .collect(),
        };

        // 解析所有有效的公式欄位
        let mut compiled: HashMap<FieldRef, Compiled> = HashMap::new();
        let mut graph = Vec::new();
        let tables = std::iter::once((None, master_fields))
            .chain(details.iter().map(|(k, f)| (Some((*k).to_owned()), *f)));
        for (detail, fields) in tables {
            for f in fields
                .iter()
                .filter(|f| f.is_active && f.data_type == DataType::Formula)
            {
                let expr = match f.formula.as_deref() {
                    Some(text) => parse(text).map_err(|e| e.to_string()),
                    None => Err("公式欄位沒有公式，請聯絡管理者".to_owned()),
                };
                let field = FieldRef {
                    detail: detail.clone(),
                    field: f.field_key.clone(),
                };
                let deps = expr.as_ref().map(dependencies).unwrap_or_default();
                graph.push(FormulaField {
                    field: field.clone(),
                    deps,
                });
                compiled.insert(
                    field,
                    Compiled {
                        field_key: f.field_key.clone(),
                        expr,
                        result_type: f.formula_result_type,
                        decimal_places: f.decimal_places,
                    },
                );
            }
        }

        let order = calculation_order(&graph).unwrap_or_else(|cycle| {
            // 定義時已檢查循環；資料不一致時所有公式都記錄錯誤，依原本的順序
            let message = cycle.message();
            for c in compiled.values_mut() {
                c.expr = Err(message.clone());
            }
            graph.iter().map(|f| f.field.clone()).collect()
        });
        for field in order {
            let Some(c) = compiled.remove(&field) else {
                continue;
            };
            let table = match &field.detail {
                None => &mut out.master,
                Some(d) => match out.details.get_mut(d) {
                    Some(t) => t,
                    None => continue,
                },
            };
            if let Ok(expr) = &c.expr {
                for dep in dependencies(expr) {
                    if let hammer_formula::Dependency::Relation { relation, .. } = dep {
                        table.used_relations.insert(relation);
                    }
                }
            }
            table.formulas.push(c);
        }
        out
    }

    /// 沒有任何有效的公式欄位。
    pub fn is_empty(&self) -> bool {
        self.master.formulas.is_empty() && self.details.values().all(|t| t.formulas.is_empty())
    }

    /// 計算明細子表 `detail` 的一列明細，結果寫入 `data`，回傳失敗的欄位。不是有效的明細子表時不計算。
    pub fn compute_line(
        &self,
        detail: &str,
        data: &mut Map<String, Value>,
        targets: &Targets,
        today: Date,
    ) -> FormulaErrors {
        match self.details.get(detail) {
            Some(table) => compute_table(table, data, targets, &HashMap::new(), today),
            None => FormulaErrors::new(),
        }
    }

    /// 計算主表的公式，結果寫入 `data`，回傳失敗的欄位。
    ///
    /// `lines` 為有效明細子表代碼 → 該子表所有明細的 `data`（依順序，已計算明細的公式）；
    /// 沒有列出的明細子表視為沒有明細。
    pub fn compute_master(
        &self,
        data: &mut Map<String, Value>,
        lines: &BTreeMap<&str, Vec<&Map<String, Value>>>,
        targets: &Targets,
        today: Date,
    ) -> FormulaErrors {
        let details: HashMap<&str, DetailRows<'_>> = self
            .details
            .iter()
            .map(|(key, table)| {
                (
                    key.as_str(),
                    DetailRows {
                        types: &table.types,
                        rows: lines.get(key.as_str()).cloned().unwrap_or_default(),
                    },
                )
            })
            .collect();
        compute_table(&self.master, data, targets, &details, today)
    }

    /// 公式的關聯引用要讀取的資料：目標資料類別 → 資料 id。
    ///
    /// `master` 為主表的 `data`；`lines` 為 `(明細子表代碼, 一列明細的 data)`。包含所有被公式引用的
    /// 關聯欄位的目標資料類別（關聯欄位沒有值時仍需要目標欄位的型態）；資料 id 只包含有值的關聯欄位。
    pub fn targets<'a>(
        &self,
        master: &Map<String, Value>,
        lines: impl IntoIterator<Item = (&'a str, &'a Map<String, Value>)>,
    ) -> BTreeMap<Id, BTreeSet<Id>> {
        let mut wanted: BTreeMap<Id, BTreeSet<Id>> = BTreeMap::new();
        for table in std::iter::once(&self.master).chain(self.details.values()) {
            for rel in &table.used_relations {
                if let Some(entity) = table.relations.get(rel) {
                    wanted.entry(*entity).or_default();
                }
            }
        }
        let pairs = self.targets_of(None, master).into_iter().chain(
            lines
                .into_iter()
                .flat_map(|(detail, data)| self.targets_of(Some(detail), data)),
        );
        for (entity, id) in pairs {
            wanted.entry(entity).or_default().insert(id);
        }
        wanted
    }

    /// 公式要讀取的關聯目標：`(目標資料類別, 資料 id)`。`table` 為 `None` 時是主表。
    fn targets_of(&self, table: Option<&str>, data: &Map<String, Value>) -> Vec<(Id, Id)> {
        let schema = match table {
            None => &self.master,
            Some(d) => match self.details.get(d) {
                Some(t) => t,
                None => return Vec::new(),
            },
        };
        schema
            .used_relations
            .iter()
            .filter_map(|rel| {
                let entity = *schema.relations.get(rel)?;
                Some((entity, relation_id(data.get(rel)?)?))
            })
            .collect()
    }
}

/// 關聯引用所指的資料：目標資料類別的欄位型態與資料（含已軟刪除）。由 [`load_targets`] 讀取；
/// 單元測試以 [`Targets::insert_entity`]、[`Targets::insert_record`] 建構。
#[derive(Debug, Clone, Default)]
pub struct Targets {
    /// 資料類別 → 有效欄位的代碼 → 型態。
    types: HashMap<Id, HashMap<String, DataType>>,
    /// `(資料類別, 資料 id)` → `data`。
    records: HashMap<(Id, Id), Map<String, Value>>,
}

impl Targets {
    /// 加入目標資料類別的欄位定義。
    pub fn insert_entity(&mut self, entity_id: Id, fields: &[FieldDefinition]) {
        self.types.insert(entity_id, TableSchema::new(fields).types);
    }

    /// 加入一筆目標資料。
    pub fn insert_record(&mut self, entity_id: Id, id: Id, data: Map<String, Value>) {
        self.records.insert((entity_id, id), data);
    }
}

/// 讀取關聯引用所需的目標資料（見 [`Targets`]）。`wanted` 為 [`Formulas::targets`] 的結果。
///
/// 讀取的是目前的值（含已軟刪除的資料），不鎖定。
pub async fn load_targets(
    conn: &mut PgConnection,
    wanted: BTreeMap<Id, BTreeSet<Id>>,
) -> Result<Targets, CoreError> {
    let mut targets = Targets::default();
    for (entity_id, ids) in wanted {
        let fields = schema_db::list_fields(&mut *conn, entity_id)
            .await?
            .into_iter()
            .map(field_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        targets.insert_entity(entity_id, &fields);
        if ids.is_empty() {
            continue;
        }
        let ids: Vec<Id> = ids.into_iter().collect();
        for row in record_db::find_records_by_ids(&mut *conn, entity_id, &ids).await? {
            if let Value::Object(data) = row.data {
                targets.insert_record(entity_id, row.id, data);
            }
        }
    }
    Ok(targets)
}

/// 主表的子表彙總讀取的一個明細子表。
struct DetailRows<'a> {
    types: &'a HashMap<String, DataType>,
    rows: Vec<&'a Map<String, Value>>,
}

/// 依計算順序計算一張表的公式，每算完一個欄位就寫入 `data`，後面的公式讀到的是新的值。
fn compute_table(
    table: &TableSchema,
    data: &mut Map<String, Value>,
    targets: &Targets,
    details: &HashMap<&str, DetailRows<'_>>,
    today: Date,
) -> FormulaErrors {
    let mut errors = FormulaErrors::new();
    for formula in &table.formulas {
        let result = match (&formula.expr, formula.result_type) {
            (Err(message), _) => Err(message.clone()),
            (Ok(_), None) => Err(MSG_NO_RESULT_TYPE.to_owned()),
            (Ok(expr), Some(result_type)) => {
                let env = RowEnv {
                    today,
                    table,
                    data,
                    targets,
                    details,
                };
                let computed = compute(expr, &env, result_type, formula.decimal_places);
                match computed.error {
                    None => Ok(computed.value),
                    Some(e) => Err(e.message()),
                }
            }
        };
        match result {
            Ok(value) if has_value(&value) => {
                data.insert(formula.field_key.clone(), value);
            }
            Ok(_) => {
                data.remove(&formula.field_key);
            }
            Err(message) => {
                data.remove(&formula.field_key);
                errors.insert(formula.field_key.clone(), message);
            }
        }
    }
    errors
}

/// 與一般欄位相同：`null` 與空字串視為沒有值，不存入 `data`。
fn has_value(value: &Value) -> bool {
    !(value.is_null() || value.as_str() == Some(""))
}

fn relation_id(value: &Value) -> Option<Id> {
    match parse_value(DataType::Relation, value) {
        Ok(Some(FieldValue::Relation(id))) => Some(id),
        _ => None,
    }
}

/// 一列資料的計算環境。
struct RowEnv<'a> {
    today: Date,
    table: &'a TableSchema,
    data: &'a Map<String, Value>,
    targets: &'a Targets,
    details: &'a HashMap<&'a str, DetailRows<'a>>,
}

static JSON_NULL: Value = Value::Null;

impl Environment for RowEnv<'_> {
    fn today(&self) -> Date {
        self.today
    }

    fn field(&self, key: &str) -> Option<FieldData<'_>> {
        let data_type = *self.table.types.get(key)?;
        Some(FieldData::new(
            data_type,
            self.data.get(key).unwrap_or(&JSON_NULL),
        ))
    }

    fn relation_field(&self, relation: &str, field: &str) -> Option<FieldData<'_>> {
        let entity = *self.table.relations.get(relation)?;
        let data_type = *self.targets.types.get(&entity)?.get(field)?;
        // 關聯欄位沒有值、或找不到目標資料時為空值
        let json = self
            .data
            .get(relation)
            .and_then(relation_id)
            .and_then(|id| self.targets.records.get(&(entity, id)))
            .and_then(|record| record.get(field))
            .unwrap_or(&JSON_NULL);
        Some(FieldData::new(data_type, json))
    }

    fn detail_column(&self, detail: &str, field: &str) -> Option<DetailColumn<'_>> {
        let rows = self.details.get(detail)?;
        let data_type = *rows.types.get(field)?;
        Some(DetailColumn {
            data_type,
            values: rows
                .rows
                .iter()
                .map(|r| r.get(field).unwrap_or(&JSON_NULL))
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hammer_shared::new_id;
    use hammer_shared::schema::FieldStorage;
    use serde_json::json;
    use time::macros::{date, datetime};

    fn field(key: &str, data_type: DataType) -> FieldDefinition {
        FieldDefinition {
            id: new_id(),
            entity_id: Id::nil(),
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
            decimal_places: matches!(data_type, DataType::Decimal | DataType::Money).then_some(2),
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

    fn formula(key: &str, text: &str, result: DataType) -> FieldDefinition {
        FieldDefinition {
            formula: Some(text.into()),
            formula_result_type: Some(result),
            decimal_places: matches!(result, DataType::Decimal | DataType::Money).then_some(2),
            ..field(key, DataType::Formula)
        }
    }

    fn relation(key: &str, target: Id) -> FieldDefinition {
        FieldDefinition {
            relation_entity_id: Some(target),
            ..field(key, DataType::Relation)
        }
    }

    fn obj(v: Value) -> Map<String, Value> {
        match v {
            Value::Object(m) => m,
            other => panic!("不是物件：{other}"),
        }
    }

    const TODAY: Date = date!(2026 - 11 - 16);

    #[test]
    fn today_uses_taipei_date() {
        // UTC 11/15 16:00 = 台北 11/16 00:00
        assert_eq!(today_of(datetime!(2026-11-15 16:00 UTC)), TODAY);
        assert_eq!(
            today_of(datetime!(2026-11-15 15:59 UTC)),
            date!(2026 - 11 - 15)
        );
    }

    #[test]
    fn errors_json_round_trip() {
        assert_eq!(errors_to_json(&FormulaErrors::new()), None);
        let errors = FormulaErrors::from([("a".to_owned(), "除數不可為零".to_owned())]);
        let json = errors_to_json(&errors).unwrap();
        assert_eq!(json, json!({ "a": "除數不可為零" }));
        assert_eq!(errors_from_json(Some(&json)), errors);
        assert_eq!(errors_from_json(None), FormulaErrors::new());
        assert_eq!(
            errors_from_json(Some(&json!({ "a": 1, "b": "x" }))),
            FormulaErrors::from([("b".to_owned(), "x".to_owned())])
        );
    }

    #[test]
    fn computes_in_dependency_order_and_ignores_inactive() {
        let mut old = formula("old", "1", DataType::Integer);
        old.is_active = false;
        // total 依賴 subtotal，但定義在前面
        let fields = [
            formula("total", "{subtotal} + 1", DataType::Money),
            formula("subtotal", "{qty} * {price}", DataType::Money),
            field("qty", DataType::Integer),
            field("price", DataType::Money),
            old,
        ];
        let f = Formulas::new(&fields, &[]);
        assert!(!f.is_empty());
        let mut data = obj(json!({ "qty": 3, "price": "2.50", "subtotal": "999", "old": 5 }));
        let errors = f.compute_master(&mut data, &BTreeMap::new(), &Targets::default(), TODAY);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            Value::Object(data),
            json!({ "qty": 3, "price": "2.50", "subtotal": "7.50", "total": "8.50", "old": 5 })
        );
    }

    #[test]
    fn no_formulas_is_empty() {
        let mut inactive = formula("x", "1", DataType::Integer);
        inactive.is_active = false;
        let master = [field("qty", DataType::Integer), inactive];
        let detail = [field("amount", DataType::Money)];
        assert!(Formulas::new(&master, &[("parts", &detail)]).is_empty());
    }

    #[test]
    fn errors_store_null_and_reason() {
        let fields = [
            formula("ratio", "{a} / {b}", DataType::Decimal),
            formula("next", "COALESCE({ratio}, 0) + 1", DataType::Integer),
            field("a", DataType::Integer),
            field("b", DataType::Integer),
        ];
        let f = Formulas::new(&fields, &[]);
        let mut data = obj(json!({ "a": 1, "b": 0, "ratio": "9.00" }));
        let errors = f.compute_master(&mut data, &BTreeMap::new(), &Targets::default(), TODAY);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors["ratio"], "除數不可為零");
        // 失敗的欄位沒有值，後面的公式讀到空值
        assert_eq!(Value::Object(data), json!({ "a": 1, "b": 0, "next": 1 }));
    }

    #[test]
    fn null_and_empty_text_results_are_removed() {
        let fields = [
            formula("n", "{a} + 1", DataType::Integer),
            formula("t", "{s} & \"\"", DataType::Text),
            field("a", DataType::Integer),
            field("s", DataType::Text),
        ];
        let f = Formulas::new(&fields, &[]);
        let mut data = obj(json!({ "n": 5, "t": "x" }));
        let errors = f.compute_master(&mut data, &BTreeMap::new(), &Targets::default(), TODAY);
        assert!(errors.is_empty());
        assert_eq!(Value::Object(data), json!({}));
    }

    #[test]
    fn today_comes_from_argument() {
        let fields = [formula("d", "TODAY()", DataType::Date)];
        let f = Formulas::new(&fields, &[]);
        let mut data = Map::new();
        f.compute_master(&mut data, &BTreeMap::new(), &Targets::default(), TODAY);
        assert_eq!(data["d"], json!("2026-11-16"));
    }

    #[test]
    fn lines_then_master_aggregates() {
        let master = [
            formula("total", "SUM(parts.amount)", DataType::Money),
            formula("count", "COUNT(parts.amount)", DataType::Integer),
        ];
        let detail = [
            formula("amount", "{qty} * {price}", DataType::Money),
            field("qty", DataType::Integer),
            field("price", DataType::Money),
        ];
        let f = Formulas::new(&master, &[("parts", &detail)]);
        let targets = Targets::default();
        let mut l1 = obj(json!({ "qty": 2, "price": "1.25" }));
        let mut l2 = obj(json!({ "qty": 1, "price": "10.00" }));
        assert!(f.compute_line("parts", &mut l1, &targets, TODAY).is_empty());
        assert!(f.compute_line("parts", &mut l2, &targets, TODAY).is_empty());
        assert_eq!(l1["amount"], json!("2.50"));

        let mut data = Map::new();
        let lines = BTreeMap::from([("parts", vec![&l1, &l2])]);
        assert!(
            f.compute_master(&mut data, &lines, &targets, TODAY)
                .is_empty()
        );
        assert_eq!(Value::Object(data), json!({ "total": "12.50", "count": 2 }));

        // 沒有明細
        let mut data = Map::new();
        f.compute_master(&mut data, &BTreeMap::new(), &targets, TODAY);
        assert_eq!(Value::Object(data), json!({ "total": "0.00", "count": 0 }));

        // 不是有效的明細子表時不計算
        let mut other = obj(json!({ "qty": 1 }));
        assert!(
            f.compute_line("other", &mut other, &targets, TODAY)
                .is_empty()
        );
        assert_eq!(Value::Object(other), json!({ "qty": 1 }));
    }

    #[test]
    fn relation_reads_target_values() {
        let customer = new_id();
        let master = [
            relation("customer", customer),
            formula(
                "price",
                "{base} * (1 - {customer.discount})",
                DataType::Money,
            ),
            field("base", DataType::Money),
        ];
        let f = Formulas::new(&master, &[]);
        let target_fields = [field("discount", DataType::Decimal)];
        let c1 = new_id();
        let mut targets = Targets::default();
        targets.insert_entity(customer, &target_fields);
        targets.insert_record(customer, c1, obj(json!({ "discount": "0.10" })));
        let wanted = f.targets(&obj(json!({ "customer": c1 })), []);
        assert_eq!(wanted, BTreeMap::from([(customer, BTreeSet::from([c1]))]));
        // 關聯欄位沒有值：仍讀取目標資料類別的欄位型態
        assert_eq!(
            f.targets(&obj(json!({})), []),
            BTreeMap::from([(customer, BTreeSet::new())])
        );

        let mut data = obj(json!({ "customer": c1, "base": "100.00" }));
        assert!(
            f.compute_master(&mut data, &BTreeMap::new(), &targets, TODAY)
                .is_empty()
        );
        assert_eq!(data["price"], json!("90.00"));

        // 關聯沒有值：目標欄位為空值 → 結果為空值
        let mut data = obj(json!({ "base": "100.00" }));
        assert!(
            f.compute_master(&mut data, &BTreeMap::new(), &targets, TODAY)
                .is_empty()
        );
        assert!(!data.contains_key("price"));

        // 目標欄位不存在（例如已停用）→ 錯誤
        let mut targets = Targets::default();
        targets.insert_entity(customer, &[]);
        let mut data = obj(json!({ "customer": c1, "base": "100.00" }));
        let errors = f.compute_master(&mut data, &BTreeMap::new(), &targets, TODAY);
        assert!(errors.contains_key("price"), "{errors:?}");
    }

    #[test]
    fn line_relation_targets() {
        let target = new_id();
        let detail = [
            relation("part", target),
            formula("name", "{part.name}", DataType::Text),
        ];
        let f = Formulas::new(&[], &[("parts", &detail)]);
        let id = new_id();
        let line = obj(json!({ "part": id }));
        let id2 = new_id();
        let line2 = obj(json!({ "part": id2 }));
        assert_eq!(
            f.targets(
                &Map::new(),
                [("parts", &line), ("parts", &line2), ("parts", &line)]
            ),
            BTreeMap::from([(target, BTreeSet::from([id, id2]))])
        );
        assert_eq!(
            f.targets(&Map::new(), [("other", &line)]),
            BTreeMap::from([(target, BTreeSet::new())])
        );
        // 沒有引用關聯的公式
        let plain = Formulas::new(&[formula("a", "1", DataType::Integer)], &[]);
        assert!(plain.targets(&Map::new(), []).is_empty());
    }

    #[test]
    fn invalid_definitions_are_errors_not_failures() {
        let mut no_type = formula("a", "1", DataType::Integer);
        no_type.formula_result_type = None;
        let mut no_formula = formula("b", "1", DataType::Integer);
        no_formula.formula = None;
        let fields = [
            no_type,
            no_formula,
            formula("c", "1 +", DataType::Integer),
            formula("d", "{missing}", DataType::Integer),
        ];
        let f = Formulas::new(&fields, &[]);
        let mut data = Map::new();
        let errors = f.compute_master(&mut data, &BTreeMap::new(), &Targets::default(), TODAY);
        assert_eq!(
            errors.keys().map(String::as_str).collect::<Vec<_>>(),
            ["a", "b", "c", "d"]
        );
        assert_eq!(errors["a"], MSG_NO_RESULT_TYPE);
        assert!(data.is_empty());
    }

    #[test]
    fn cycle_marks_all_formulas() {
        let fields = [
            formula("a", "{b}", DataType::Integer),
            formula("b", "{a}", DataType::Integer),
            formula("c", "1", DataType::Integer),
        ];
        let f = Formulas::new(&fields, &[]);
        let mut data = Map::new();
        let errors = f.compute_master(&mut data, &BTreeMap::new(), &Targets::default(), TODAY);
        assert_eq!(errors.len(), 3);
        assert!(errors["c"].contains("循環"), "{errors:?}");
    }

    #[test]
    fn value_type_of_formula_is_result_type() {
        assert_eq!(
            value_type(&formula("a", "1", DataType::Money)),
            Some(DataType::Money)
        );
        let mut f = formula("a", "1", DataType::Money);
        f.formula_result_type = None;
        assert_eq!(value_type(&f), None);
        assert_eq!(
            value_type(&field("t", DataType::Text)),
            Some(DataType::Text)
        );
    }
}

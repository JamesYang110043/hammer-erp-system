//! 公式欄位的定義檢查（docs/architecture.md §7.6、§8.5 `formula_deps`）。
//!
//! 儲存公式欄位時（[`validate_formula`]）依序：
//!
//! 1. 語法（[`hammer_formula::parse`]）
//! 2. 讀取需要的欄位定義（[`load_context`]）：公式所在資料表的有效欄位；明細子表的公式另讀主表的
//!    欄位代碼（錯誤訊息用）；公式引用的關聯欄位所指資料類別的有效欄位；主表的公式引用的有效明細
//!    子表的有效欄位；整個資料類別（主表與有效的明細子表）其他有效公式欄位的 `formula_deps`
//! 3. 引用與型態（[`hammer_formula::check_formula`]）：引用的欄位必須存在且有效，結果型態相符
//! 4. 循環引用（[`hammer_formula::calculation_order`]）：以整個資料類別的公式欄位重建相依圖
//!
//! 錯誤都是 [`CoreError::InvalidFormula`]（`details.field = formula`），帶錯誤在公式中的字元位置
//! 或循環路徑。通過後回傳要存入 `formula_deps` 的 JSON。
//!
//! 被引用的資料類別（關聯目標、明細子表）以 `FOR SHARE` 鎖定：停用欄位、停用明細子表時以
//! `FOR UPDATE` 鎖定所屬資料類別，所以檢查後引用的欄位不會被停用（停用時的引用檢查見
//! [`super::deactivate_field`]）。
//!
//! 明細子表的公式不能引用主表欄位，所以循環只可能發生在同一張表內；以公式所在資料類別的
//! `FOR UPDATE` 鎖定就能讓同一張表的公式變更依序進行。

use std::collections::{BTreeSet, HashMap};

use hammer_db::schema as db;
use hammer_db::sqlx::PgTransaction;
use hammer_formula::{
    CycleError, Dependency, Expr, FieldRef, FormulaField, ParseError, TypeError, TypeSchema,
    calculation_order, check_formula, dependencies, deps_from_json, deps_to_json, parse,
};
use hammer_shared::Id;
use hammer_shared::schema::{DataType, EntityDefinition, EntityKind, FieldDefinition};
use serde_json::Value;

use crate::CoreError;

use super::{entity_from_row, field_from_row};

/// 公式不合法的原因與位置（[`CoreError::InvalidFormula`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormulaError {
    /// 顯示給使用者的訊息（繁體中文）。
    pub message: String,
    pub location: FormulaErrorLocation,
}

/// 錯誤的位置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormulaErrorLocation {
    /// 錯誤在公式文字中的位置（字元位移，從 0 開始，不含 `end`）。
    Span { start: usize, end: usize },
    /// 循環引用的路徑，第一個與最後一個相同；明細欄位為 `明細子表代碼.欄位代碼`。
    Cycle(Vec<String>),
}

impl From<ParseError> for FormulaError {
    fn from(e: ParseError) -> Self {
        Self {
            message: e.to_string(),
            location: FormulaErrorLocation::Span {
                start: e.span.start,
                end: e.span.end,
            },
        }
    }
}

impl From<TypeError> for FormulaError {
    fn from(e: TypeError) -> Self {
        Self {
            message: e.to_string(),
            location: FormulaErrorLocation::Span {
                start: e.span.start,
                end: e.span.end,
            },
        }
    }
}

impl From<CycleError> for FormulaError {
    fn from(e: CycleError) -> Self {
        Self {
            message: e.message(),
            location: FormulaErrorLocation::Cycle(e.path.iter().map(ToString::to_string).collect()),
        }
    }
}

macro_rules! into_core_error {
    ($($t:ty),*) => {$(
        impl From<$t> for CoreError {
            fn from(e: $t) -> Self {
                CoreError::InvalidFormula(e.into())
            }
        }
    )*};
}

into_core_error!(ParseError, TypeError, CycleError);

/// 檢查公式需要的欄位定義（[`load_context`] 從資料庫讀取；單元測試直接建構）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FormulaContext {
    /// 公式在明細子表時為子表代碼；在主表時為 `None`。
    pub detail_key: Option<String>,
    /// 公式所在資料表的欄位（可含已停用，檢查時略過）。正在定義的欄位如果已存在，以新的定義為準。
    pub fields: Vec<FieldDefinition>,
    /// 明細子表的公式：主表的有效欄位代碼（「不能引用主表欄位」的錯誤訊息用）。
    pub master_fields: Vec<String>,
    /// 關聯欄位代碼 → 目標資料類別的欄位（可含已停用）。只包含目標有效的關聯欄位。
    pub relations: HashMap<String, Vec<FieldDefinition>>,
    /// 明細子表代碼 → 子表的欄位（可含已停用）。只包含主表有效的明細子表；明細的公式為空。
    pub details: HashMap<String, Vec<FieldDefinition>>,
    /// 同一個資料類別（主表與有效的明細子表）有效公式欄位的相依。正在定義的欄位如果已存在，
    /// 以新的公式為準。
    pub formulas: Vec<FormulaField>,
}

/// 解析公式欄位的公式。`def.formula` 已由 [`super::rules::validate_field_config`] 檢查不為空。
pub fn parse_formula(def: &FieldDefinition) -> Result<Expr, CoreError> {
    let text = def
        .formula
        .as_deref()
        .ok_or_else(|| CoreError::Internal("公式欄位沒有公式".into()))?;
    Ok(parse(text)?)
}

/// 檢查公式的引用、型態與循環引用（不需要資料庫的部分）。
pub fn check(expr: &Expr, def: &FieldDefinition, ctx: &FormulaContext) -> Result<(), CoreError> {
    let result_type = def
        .formula_result_type
        .ok_or_else(|| CoreError::Internal("公式欄位沒有結果型態".into()))?;
    let mut schema = match ctx.detail_key {
        None => TypeSchema::master(),
        Some(_) => TypeSchema::detail(ctx.master_fields.iter().cloned()),
    };
    schema.fields = field_types(ctx.fields.iter().filter(|f| f.field_key != def.field_key));
    // 自己也在欄位中：引用自己時回報循環引用，而不是「找不到欄位」
    schema.fields.insert(def.field_key.clone(), result_type);
    schema.relations = ctx
        .relations
        .iter()
        .map(|(k, fields)| (k.clone(), field_types(fields.iter())))
        .collect();
    if ctx.detail_key.is_none() {
        schema.details = ctx
            .details
            .iter()
            .map(|(k, fields)| (k.clone(), field_types(fields.iter())))
            .collect();
    }
    check_formula(expr, &schema, result_type)?;

    let me = FieldRef {
        detail: ctx.detail_key.clone(),
        field: def.field_key.clone(),
    };
    let mut formulas = vec![FormulaField {
        field: me.clone(),
        deps: dependencies(expr),
    }];
    formulas.extend(ctx.formulas.iter().filter(|f| f.field != me).cloned());
    calculation_order(&formulas)?;
    Ok(())
}

/// 有效欄位在公式中的型態：代碼 → 型態；`formula` 欄位為它的結果型態。
fn field_types<'a>(fields: impl Iterator<Item = &'a FieldDefinition>) -> HashMap<String, DataType> {
    fields
        .filter(|f| f.is_active)
        .filter_map(|f| {
            let t = match f.data_type {
                DataType::Formula => f.formula_result_type?,
                t => t,
            };
            Some((f.field_key.clone(), t))
        })
        .collect()
}

/// 檢查公式欄位並回傳 `formula_deps`（JSON 陣列）。
///
/// `full = false` 時只檢查語法（已停用的公式欄位：重新啟用時才做完整檢查）。
/// `entity` 為公式所在的資料類別，呼叫端已以 `FOR UPDATE` 鎖定。
pub async fn validate_formula(
    tx: &mut PgTransaction<'_>,
    entity: &EntityDefinition,
    def: &FieldDefinition,
    full: bool,
) -> Result<Value, CoreError> {
    let expr = parse_formula(def)?;
    let deps = dependencies(&expr);
    if full {
        let ctx = load_context(tx, entity, &deps).await?;
        check(&expr, def, &ctx)?;
    }
    Ok(deps_to_json(&deps))
}

/// 從資料庫讀取檢查公式需要的欄位定義（見模組說明）。`deps` 為公式的相依。
pub async fn load_context(
    tx: &mut PgTransaction<'_>,
    entity: &EntityDefinition,
    deps: &[Dependency],
) -> Result<FormulaContext, CoreError> {
    let fields = load_fields(tx, entity.id).await?;

    // 主表（資料類別的根）
    let (root, detail_key, master_fields) = match entity.kind {
        EntityKind::Detail => {
            let parent_id = entity
                .parent_entity_id
                .ok_or_else(|| CoreError::Internal("明細子表沒有主表".into()))?;
            let parent = db::find_entity_by_id(&mut **tx, parent_id)
                .await?
                .map(entity_from_row)
                .transpose()?
                .ok_or_else(|| CoreError::Internal("找不到明細子表的主表".into()))?;
            let master_fields = load_fields(tx, parent.id)
                .await?
                .into_iter()
                .filter(|f| f.is_active)
                .map(|f| f.field_key)
                .collect();
            (parent, Some(entity.key.clone()), master_fields)
        }
        EntityKind::Master | EntityKind::Document => (entity.clone(), None, Vec::new()),
    };

    // 有效的明細子表；以 FOR SHARE 鎖定，避免檢查後被停用
    let details: Vec<EntityDefinition> = db::list_detail_entities_for_share(&mut **tx, root.id)
        .await?
        .into_iter()
        .map(entity_from_row)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|d| d.is_active)
        .collect();

    // 主表的公式引用的明細子表
    let mut detail_fields = HashMap::new();
    if detail_key.is_none() {
        let used: BTreeSet<&str> = deps
            .iter()
            .filter_map(|d| match d {
                Dependency::Detail { detail, .. } => Some(detail.as_str()),
                _ => None,
            })
            .collect();
        for d in details.iter().filter(|d| used.contains(d.key.as_str())) {
            detail_fields.insert(d.key.clone(), load_fields(tx, d.id).await?);
        }
    }

    // 公式引用的關聯欄位所指的資料類別；以 FOR SHARE 鎖定，避免檢查後欄位被停用
    let used: BTreeSet<&str> = deps
        .iter()
        .filter_map(|d| match d {
            Dependency::Relation { relation, .. } => Some(relation.as_str()),
            _ => None,
        })
        .collect();
    let mut relations = HashMap::new();
    for f in fields.iter().filter(|f| {
        f.is_active && f.data_type == DataType::Relation && used.contains(f.field_key.as_str())
    }) {
        let Some(target_id) = f.relation_entity_id else {
            continue;
        };
        let Some(target) = db::find_entity_by_id_for_share(&mut **tx, target_id)
            .await?
            .map(entity_from_row)
            .transpose()?
        else {
            continue;
        };
        if target.is_active && target.kind != EntityKind::Detail {
            relations.insert(f.field_key.clone(), load_fields(tx, target.id).await?);
        }
    }

    // 整個資料類別的公式欄位（循環檢查）
    let keys: HashMap<Id, Option<String>> = std::iter::once((root.id, None))
        .chain(details.iter().map(|d| (d.id, Some(d.key.clone()))))
        .collect();
    let ids: Vec<Id> = keys.keys().copied().collect();
    let mut formulas = Vec::new();
    for row in db::list_formula_deps(&mut **tx, &ids).await? {
        let deps = match row.formula_deps {
            None | Some(Value::Null) => Vec::new(),
            Some(json) => deps_from_json(&json).map_err(|e| {
                CoreError::Internal(format!("field_definitions.formula_deps 格式錯誤：{e}"))
            })?,
        };
        formulas.push(FormulaField {
            field: FieldRef {
                detail: keys.get(&row.entity_id).cloned().flatten(),
                field: row.field_key,
            },
            deps,
        });
    }
    // 依明細子表代碼、欄位排序，讓回報的循環路徑固定
    formulas.sort_by(|a, b| a.field.cmp(&b.field));

    Ok(FormulaContext {
        detail_key,
        fields,
        master_fields,
        relations,
        details: detail_fields,
        formulas,
    })
}

async fn load_fields(
    tx: &mut PgTransaction<'_>,
    entity_id: Id,
) -> Result<Vec<FieldDefinition>, CoreError> {
    db::list_fields(&mut **tx, entity_id)
        .await?
        .into_iter()
        .map(field_from_row)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hammer_shared::ErrorCode;
    use hammer_shared::new_id;
    use hammer_shared::schema::FieldStorage;
    use time::macros::datetime;

    fn field(key: &str, data_type: DataType) -> FieldDefinition {
        let now = datetime!(2026-11-16 09:00 +8);
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
            sort_order: 0,
            is_active: true,
            created_at: now,
            updated_at: now,
            version: 1,
        }
    }

    fn formula(key: &str, text: &str, result: DataType) -> FieldDefinition {
        FieldDefinition {
            formula: Some(text.into()),
            formula_result_type: Some(result),
            ..field(key, DataType::Formula)
        }
    }

    fn inactive(mut f: FieldDefinition) -> FieldDefinition {
        f.is_active = false;
        f
    }

    fn master_ctx() -> FormulaContext {
        FormulaContext {
            fields: vec![
                field("qty", DataType::Integer),
                field("price", DataType::Money),
                field("note", DataType::Text),
                field("customer", DataType::Relation),
                inactive(field("old", DataType::Integer)),
                formula("subtotal", "{qty} * {price}", DataType::Money),
            ],
            relations: HashMap::from([(
                "customer".to_owned(),
                vec![
                    field("discount", DataType::Decimal),
                    inactive(field("legacy", DataType::Decimal)),
                ],
            )]),
            details: HashMap::from([(
                "lines".to_owned(),
                vec![
                    field("amount", DataType::Money),
                    field("memo", DataType::Text),
                    inactive(field("gone", DataType::Money)),
                    formula("line_total", "{amount}", DataType::Money),
                ],
            )]),
            formulas: vec![
                FormulaField {
                    field: FieldRef::master("subtotal"),
                    deps: vec![
                        Dependency::Field {
                            field: "price".into(),
                        },
                        Dependency::Field {
                            field: "qty".into(),
                        },
                    ],
                },
                FormulaField {
                    field: FieldRef::detail("lines", "line_total"),
                    deps: vec![Dependency::Field {
                        field: "amount".into(),
                    }],
                },
            ],
            ..FormulaContext::default()
        }
    }

    fn run(def: &FieldDefinition, ctx: &FormulaContext) -> Result<(), CoreError> {
        let expr = parse_formula(def)?;
        check(&expr, def, ctx)
    }

    fn err_of(def: &FieldDefinition, ctx: &FormulaContext) -> FormulaError {
        match run(def, ctx).unwrap_err() {
            CoreError::InvalidFormula(e) => e,
            other => panic!("不是公式錯誤：{other:?}"),
        }
    }

    fn span_of(e: &FormulaError) -> (usize, usize) {
        match e.location {
            FormulaErrorLocation::Span { start, end } => (start, end),
            ref other => panic!("沒有位置：{other:?}"),
        }
    }

    #[test]
    fn accepts_field_relation_and_aggregate_references() {
        let ctx = master_ctx();
        for (text, result) in [
            ("{qty} * {price}", DataType::Money),
            ("{subtotal} * (1 - {customer.discount})", DataType::Money),
            ("SUM(lines.amount) + SUM(lines.line_total)", DataType::Money),
            ("COUNT(lines.memo)", DataType::Integer),
            ("{note} & \"!\"", DataType::Text),
        ] {
            let def = formula("total", text, result);
            assert!(run(&def, &ctx).is_ok(), "{text}");
        }
    }

    #[test]
    fn syntax_errors_have_position() {
        let e = err_of(
            &formula("total", "{qty} *", DataType::Integer),
            &master_ctx(),
        );
        assert_eq!(span_of(&e), (7, 7));
        assert!(e.message.contains("公式不完整"), "{}", e.message);
    }

    #[test]
    fn unknown_or_inactive_references_are_rejected() {
        let ctx = master_ctx();
        for (text, span) in [
            ("{missing} + 1", (0, 9)),
            ("1 + {old}", (4, 9)),
            ("{customer.legacy}", (0, 17)),
            ("{customer.nope}", (0, 15)),
            ("{note.x}", (0, 8)),
            ("SUM(lines.gone)", (0, 15)),
            ("SUM(parts.amount)", (0, 17)),
        ] {
            let e = err_of(&formula("total", text, DataType::Money), &ctx);
            assert_eq!(span_of(&e), span, "{text}: {}", e.message);
        }
    }

    #[test]
    fn type_mismatch_is_rejected() {
        let ctx = master_ctx();
        let e = err_of(&formula("total", "{note} * 2", DataType::Money), &ctx);
        assert_eq!(span_of(&e), (0, 6));
        // 結果型態不符：位置是整個公式
        let e = err_of(&formula("total", "{qty} + 1", DataType::Text), &ctx);
        assert_eq!(span_of(&e), (0, 9));
        // 彙總非數字欄位
        let e = err_of(&formula("total", "SUM(lines.memo)", DataType::Money), &ctx);
        assert_eq!(span_of(&e), (0, 15));
    }

    #[test]
    fn detail_formula_rules() {
        let ctx = FormulaContext {
            detail_key: Some("lines".into()),
            fields: vec![field("amount", DataType::Money)],
            master_fields: vec!["qty".into()],
            ..FormulaContext::default()
        };
        assert!(run(&formula("double", "{amount} * 2", DataType::Money), &ctx).is_ok());
        let e = err_of(&formula("x", "{qty}", DataType::Integer), &ctx);
        assert!(e.message.contains("主表欄位"), "{}", e.message);
        let e = err_of(&formula("x", "SUM(lines.amount)", DataType::Money), &ctx);
        assert!(e.message.contains("子表彙總"), "{}", e.message);
    }

    #[test]
    fn self_reference_is_a_cycle() {
        let e = err_of(
            &formula("total", "{total} + 1", DataType::Integer),
            &master_ctx(),
        );
        assert_eq!(
            e.location,
            FormulaErrorLocation::Cycle(vec!["total".into(), "total".into()])
        );
        assert!(e.message.contains("循環"), "{}", e.message);
    }

    #[test]
    fn cycle_through_other_formulas() {
        // subtotal 已存在：{qty} * {price}。修改 subtotal 引用 total、total 引用 subtotal
        let mut ctx = master_ctx();
        ctx.fields
            .push(formula("total", "{subtotal} + 1", DataType::Money));
        ctx.formulas.push(FormulaField {
            field: FieldRef::master("total"),
            deps: vec![Dependency::Field {
                field: "subtotal".into(),
            }],
        });
        let e = err_of(&formula("subtotal", "{total} * 2", DataType::Money), &ctx);
        assert_eq!(
            e.location,
            FormulaErrorLocation::Cycle(vec!["subtotal".into(), "total".into(), "subtotal".into()])
        );
    }

    #[test]
    fn redefining_a_formula_uses_new_definition() {
        // 已存在的 subtotal 原本是數字；改成文字結果後，以新的型態檢查
        let def = formula("subtotal", "{note}", DataType::Text);
        assert!(run(&def, &master_ctx()).is_ok());
    }

    #[test]
    fn errors_map_to_validation_with_formula_field() {
        let err = run(
            &formula("total", "{missing}", DataType::Integer),
            &master_ctx(),
        )
        .unwrap_err();
        assert_eq!(err.code(), ErrorCode::Validation);
    }
}

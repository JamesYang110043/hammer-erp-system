//! 重新計算資料類別所有資料的公式欄位（docs/architecture.md §7.6「重新計算」、§10）。
//!
//! 修改公式後，既有資料的值不會自動更新；關聯引用的目標資料修改後，引用它的公式也不會更新。
//! 管理者（`schema.manage`）以 [`recalculate_records`] 對一個主表（`master` / `document`）的所有
//! 未刪除資料重算，明細隨主表一起重算。
//!
//! - 同步執行，每 [`BATCH_SIZE`] 筆主表資料一個 transaction（依 `id` 由小到大，鎖定該批主表與明細）
//! - 計算方式與存檔時相同（[`crate::formulas`]）：先算每一列明細（只有有效的明細子表），再算主表；
//!   `TODAY()` 為執行時間在 Asia/Taipei 的日期
//! - 主表或任一列明細的值（含 `formula_errors`）有變動時寫入，主表的 `version` + 1（有變動的明細
//!   也 + 1）；都沒有變動的資料不寫入
//! - 不逐筆寫操作紀錄；整批完成時，在最後一批的 transaction 內寫一筆 `audit_logs`
//!   （動作 `recalculate`、`entity_key` 為資料類別代碼、`record_id` 為空，`after_data` 為筆數）
//!
//! 中途失敗時（例如資料類別在批次之間被停用），已完成的批次不會還原，也不會寫操作紀錄；
//! 再執行一次即可（沒有變動的資料不會寫入）。

use std::collections::{BTreeMap, BTreeSet, HashMap};

use hammer_db::DbError;
use hammer_db::record::{self as db, LineRow, RecordRow};
use hammer_db::sqlx::{PgPool, PgTransaction};
use hammer_shared::schema::RecalculateResult;
use hammer_shared::time::Date;
use hammer_shared::{Id, audit as action, permission};
use serde_json::{Map, Value, json};
use time::OffsetDateTime;

use crate::CoreError;
use crate::audit::{self, AuditDiff, AuditEntry};
use crate::auth::AuthContext;
use crate::formulas::{Formulas, Targets, errors_to_json, load_targets, today_of};
use crate::records::{Detail, formulas_of, load_details_for_write, load_entity_for_write};

/// 每個 transaction 處理的主表資料筆數。
pub const BATCH_SIZE: usize = 500;

/// 重新計算資料類別 `entity_key` 所有未刪除資料的公式欄位。需要 `schema.manage`。
///
/// - 沒有權限 → [`CoreError::Forbidden`]（先於其他檢查）
/// - 資料類別不存在、已停用、不是 `custom_records`、是明細子表 → [`CoreError::NotFound`]
pub async fn recalculate_records(
    pool: &PgPool,
    ctx: &AuthContext,
    entity_key: &str,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<RecalculateResult, CoreError> {
    ctx.require(permission::SCHEMA_MANAGE)?;
    let today = today_of(now);
    let mut result = RecalculateResult::default();
    let mut after: Option<Id> = None;
    loop {
        let mut tx = pool.begin().await.map_err(DbError::from)?;
        // 每批重新讀取欄位定義並以 FOR SHARE 鎖定資料類別，批次內不會與欄位定義的變更交錯
        let (entity, fields) = load_entity_for_write(&mut tx, entity_key).await?;
        let details = load_details_for_write(&mut tx, &entity).await?;
        let formulas = formulas_of(&fields, &details);
        let limit = i64::try_from(BATCH_SIZE).unwrap_or(i64::MAX);
        let rows = db::list_records_after_for_update(&mut *tx, entity.id, after, limit).await?;
        let last_batch = rows.len() < BATCH_SIZE;
        after = rows.last().map(|r| r.id);

        let batch = recalculate_batch(&mut tx, &formulas, &details, &rows, today).await?;
        if !batch.writes.is_empty() {
            let ids: Vec<Id> = batch.writes.iter().map(|w| w.id).collect();
            let data: Vec<Value> = batch.writes.iter().map(|w| w.data.clone()).collect();
            let errors: Vec<Value> = batch
                .writes
                .iter()
                .map(|w| w.formula_errors.clone().unwrap_or(Value::Null))
                .collect();
            db::update_formula_values(&mut *tx, &ids, &data, &errors, ctx.user_id).await?;
        }
        result.processed = result.processed.saturating_add(count(rows.len()));
        result.changed = result.changed.saturating_add(batch.changed);
        result.with_errors = result.with_errors.saturating_add(batch.with_errors);

        if last_batch {
            write_audit(&mut tx, ctx, &entity.key, &result, client_info, now).await?;
            tx.commit().await.map_err(DbError::from)?;
            tracing::info!(
                actor = %ctx.user_id,
                entity = %entity.key,
                processed = result.processed,
                changed = result.changed,
                with_errors = result.with_errors,
                "已重新計算公式"
            );
            return Ok(result);
        }
        tx.commit().await.map_err(DbError::from)?;
    }
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

async fn write_audit(
    tx: &mut PgTransaction<'_>,
    ctx: &AuthContext,
    entity_key: &str,
    result: &RecalculateResult,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<(), CoreError> {
    let after = json!({
        "processed": result.processed,
        "changed": result.changed,
        "with_errors": result.with_errors,
    });
    audit::record(
        tx,
        &AuditEntry {
            user_id: Some(ctx.user_id),
            action: action::RECALCULATE,
            entity_key: Some(entity_key),
            record_id: None,
            diff: AuditDiff::between(None, Some(&after))?,
            client_info,
            occurred_at: now,
        },
    )
    .await?;
    Ok(())
}

/// 一批主表資料的重算結果。
#[derive(Debug, Default)]
struct Batch {
    writes: Vec<Write>,
    changed: u32,
    with_errors: u32,
}

/// 重算一批主表資料（與其明細）：讀取所有關聯引用的目標後逐筆計算。
async fn recalculate_batch(
    tx: &mut PgTransaction<'_>,
    formulas: &Formulas,
    details: &[Detail],
    rows: &[RecordRow],
    today: Date,
) -> Result<Batch, CoreError> {
    if rows.is_empty() {
        return Ok(Batch::default());
    }
    let ids: Vec<Id> = rows.iter().map(|r| r.id).collect();
    let line_rows = db::list_lines_of_parents_for_update(&mut **tx, &ids).await?;

    // 只計算有效的明細子表；已停用的明細子表的明細不變
    let detail_keys: HashMap<Id, &str> = details
        .iter()
        .filter(|d| d.entity.is_active)
        .map(|d| (d.entity.id, d.entity.key.as_str()))
        .collect();
    let mut lines_of: HashMap<Id, Vec<Stored<'_>>> = HashMap::new();
    for line in &line_rows {
        if let Some(key) = detail_keys.get(&line.entity_id) {
            lines_of
                .entry(line.parent_record_id)
                .or_default()
                .push(Stored::line(key, line));
        }
    }
    let masters: Vec<(Stored<'_>, Vec<Stored<'_>>)> = rows
        .iter()
        .map(|r| {
            (
                Stored::master(r),
                lines_of.remove(&r.id).unwrap_or_default(),
            )
        })
        .collect();

    // 整批的關聯目標一次讀取
    let mut wanted: BTreeMap<Id, BTreeSet<Id>> = BTreeMap::new();
    for (master, lines) in &masters {
        let master_data = object(master.data)?;
        let line_data = lines
            .iter()
            .map(|l| Ok((l.detail.unwrap_or_default(), object(l.data)?)))
            .collect::<Result<Vec<_>, CoreError>>()?;
        for (entity, ids) in formulas.targets(master_data, line_data) {
            wanted.entry(entity).or_default().extend(ids);
        }
    }
    let targets = load_targets(tx, wanted).await?;

    let mut batch = Batch::default();
    for (master, lines) in &masters {
        let outcome = recalculate_one(formulas, master, lines, &targets, today)?;
        if !outcome.writes.is_empty() {
            batch.changed += 1;
        }
        if outcome.has_errors {
            batch.with_errors += 1;
        }
        batch.writes.extend(outcome.writes);
    }
    Ok(batch)
}

/// 資料庫中的一筆資料（主表或一列明細）。
#[derive(Debug, Clone, Copy)]
struct Stored<'a> {
    id: Id,
    /// 明細子表代碼；主表為 `None`。
    detail: Option<&'a str>,
    data: &'a Value,
    formula_errors: Option<&'a Value>,
}

impl<'a> Stored<'a> {
    fn master(row: &'a RecordRow) -> Self {
        Self {
            id: row.id,
            detail: None,
            data: &row.data,
            formula_errors: row.formula_errors.as_ref(),
        }
    }

    fn line(detail: &'a str, row: &'a LineRow) -> Self {
        Self {
            id: row.id,
            detail: Some(detail),
            data: &row.data,
            formula_errors: row.formula_errors.as_ref(),
        }
    }
}

/// 一筆要寫入的資料：取代 `data` 與 `formula_errors`，`version` + 1。
#[derive(Debug, Clone, PartialEq)]
struct Write {
    id: Id,
    data: Value,
    formula_errors: Option<Value>,
}

/// 一筆主表資料的重算結果。
#[derive(Debug, Default, PartialEq)]
struct Outcome {
    /// 要寫入的資料。主表或任一列明細有變動時，主表一定在第一筆（`version` + 1）；之後是有變動的明細。
    /// 都沒有變動時為空。
    writes: Vec<Write>,
    /// 重算後主表或任一列明細有計算錯誤。
    has_errors: bool,
}

/// 重算一筆主表資料：先算每一列明細，再算主表（子表彙總使用明細的新值）。
///
/// `lines` 為有效明細子表的所有明細，依明細子表、順序排列。
fn recalculate_one(
    formulas: &Formulas,
    master: &Stored<'_>,
    lines: &[Stored<'_>],
    targets: &Targets,
    today: Date,
) -> Result<Outcome, CoreError> {
    let mut has_errors = false;
    let mut computed: Vec<(&Stored<'_>, Map<String, Value>, Option<Value>)> =
        Vec::with_capacity(lines.len());
    for line in lines {
        let mut data = object(line.data)?.clone();
        let errors =
            formulas.compute_line(line.detail.unwrap_or_default(), &mut data, targets, today);
        has_errors |= !errors.is_empty();
        computed.push((line, data, errors_to_json(&errors)));
    }

    let mut rows: BTreeMap<&str, Vec<&Map<String, Value>>> = BTreeMap::new();
    for (line, data, _) in &computed {
        rows.entry(line.detail.unwrap_or_default())
            .or_default()
            .push(data);
    }
    let mut data = object(master.data)?.clone();
    let errors = formulas.compute_master(&mut data, &rows, targets, today);
    has_errors |= !errors.is_empty();
    let master_write = Write {
        id: master.id,
        data: Value::Object(data),
        formula_errors: errors_to_json(&errors),
    };

    let mut writes: Vec<Write> = computed
        .into_iter()
        .map(|(line, data, errors)| {
            (
                line,
                Write {
                    id: line.id,
                    data: Value::Object(data),
                    formula_errors: errors,
                },
            )
        })
        .filter(|(line, write)| changed(line, write))
        .map(|(_, write)| write)
        .collect();
    if changed(master, &master_write) || !writes.is_empty() {
        writes.insert(0, master_write);
    }
    Ok(Outcome { writes, has_errors })
}

fn changed(stored: &Stored<'_>, write: &Write) -> bool {
    *stored.data != write.data || stored.formula_errors != write.formula_errors.as_ref()
}

fn object(data: &Value) -> Result<&Map<String, Value>, CoreError> {
    data.as_object()
        .ok_or_else(|| CoreError::Internal("custom_records.data 不是 JSON 物件".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hammer_shared::new_id;
    use hammer_shared::schema::{DataType, FieldDefinition, FieldStorage};
    use time::macros::date;

    const TODAY: Date = date!(2026 - 11 - 16);

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

    /// 主表 `qty`、`total = {qty} * 2 + SUM(parts.amount)`；明細 `parts`：`n`、`amount = {n} * 10`。
    fn formulas() -> Formulas {
        let master = [
            field("qty", DataType::Integer),
            formula("total", "{qty} * 2 + SUM(parts.amount)", DataType::Integer),
        ];
        let parts = [
            field("n", DataType::Integer),
            formula("amount", "{n} * 10", DataType::Integer),
        ];
        Formulas::new(&master, &[("parts", &parts)])
    }

    fn stored<'a>(
        detail: Option<&'a str>,
        data: &'a Value,
        errors: Option<&'a Value>,
    ) -> Stored<'a> {
        Stored {
            id: new_id(),
            detail,
            data,
            formula_errors: errors,
        }
    }

    fn run(master: &Stored<'_>, lines: &[Stored<'_>]) -> Outcome {
        recalculate_one(&formulas(), master, lines, &Targets::default(), TODAY).unwrap()
    }

    #[test]
    fn unchanged_record_is_not_written() {
        let m = json!({ "qty": 3, "total": 26 });
        let l1 = json!({ "n": 1, "amount": 10 });
        let l2 = json!({ "n": 1, "amount": 10 });
        let master = stored(None, &m, None);
        let lines = [
            stored(Some("parts"), &l1, None),
            stored(Some("parts"), &l2, None),
        ];
        assert_eq!(run(&master, &lines), Outcome::default());
    }

    #[test]
    fn changed_master_is_written_without_unchanged_lines() {
        // 公式改過，既有的 total 是舊值
        let m = json!({ "qty": 3, "total": 999 });
        let l = json!({ "n": 1, "amount": 10 });
        let master = stored(None, &m, None);
        let out = run(&master, &[stored(Some("parts"), &l, None)]);
        assert_eq!(
            out.writes,
            vec![Write {
                id: master.id,
                data: json!({ "qty": 3, "total": 16 }),
                formula_errors: None,
            }]
        );
        assert!(!out.has_errors);
    }

    #[test]
    fn changed_line_writes_line_and_master() {
        // 明細的 amount 是舊值；主表的 total 恰好不變（qty 3 → 6 + 10）時，主表仍寫入（version + 1）
        let m = json!({ "qty": 3, "total": 16 });
        let stale = json!({ "n": 1, "amount": 99 });
        let master = stored(None, &m, None);
        let line = stored(Some("parts"), &stale, None);
        let out = run(&master, &[line]);
        assert_eq!(
            out.writes,
            vec![
                Write {
                    id: master.id,
                    data: m.clone(),
                    formula_errors: None,
                },
                Write {
                    id: line.id,
                    data: json!({ "n": 1, "amount": 10 }),
                    formula_errors: None,
                },
            ]
        );
    }

    #[test]
    fn master_aggregates_use_recomputed_lines() {
        let m = json!({ "qty": 1 });
        let l1 = json!({ "n": 2 });
        let l2 = json!({ "n": 3, "amount": 1 });
        let master = stored(None, &m, None);
        let out = run(
            &master,
            &[
                stored(Some("parts"), &l1, None),
                stored(Some("parts"), &l2, None),
            ],
        );
        assert_eq!(out.writes.len(), 3);
        assert_eq!(out.writes[0].data, json!({ "qty": 1, "total": 52 }));
    }

    #[test]
    fn errors_are_counted_and_cleared() {
        let master_def = [
            field("a", DataType::Integer),
            field("b", DataType::Integer),
            formula("ratio", "{a} / {b}", DataType::Decimal),
        ];
        let f = Formulas::new(&master_def, &[]);

        // 新的錯誤：寫入原因
        let m = json!({ "a": 1, "b": 0, "ratio": "9.00" });
        let master = stored(None, &m, None);
        let out = recalculate_one(&f, &master, &[], &Targets::default(), TODAY).unwrap();
        assert!(out.has_errors);
        assert_eq!(out.writes.len(), 1);
        assert_eq!(out.writes[0].data, json!({ "a": 1, "b": 0 }));
        assert!(out.writes[0].formula_errors.is_some());

        // 錯誤不變：有錯誤但不寫入
        let stored_errors = out.writes[0].formula_errors.clone().unwrap();
        let m = json!({ "a": 1, "b": 0 });
        let master = stored(None, &m, Some(&stored_errors));
        let out = recalculate_one(&f, &master, &[], &Targets::default(), TODAY).unwrap();
        assert!(out.has_errors);
        assert!(out.writes.is_empty());

        // 錯誤消失：清除 formula_errors
        let m = json!({ "a": 1, "b": 2 });
        let master = stored(None, &m, Some(&stored_errors));
        let out = recalculate_one(&f, &master, &[], &Targets::default(), TODAY).unwrap();
        assert!(!out.has_errors);
        assert_eq!(
            out.writes,
            vec![Write {
                id: master.id,
                data: json!({ "a": 1, "b": 2, "ratio": "0.50" }),
                formula_errors: None,
            }]
        );
    }

    #[test]
    fn line_errors_count_for_master() {
        let master_def = [field("qty", DataType::Integer)];
        let parts = [
            field("n", DataType::Integer),
            formula("inv", "1 / {n}", DataType::Decimal),
        ];
        let f = Formulas::new(&master_def, &[("parts", &parts)]);
        let m = json!({ "qty": 1 });
        let l = json!({ "n": 0 });
        let master = stored(None, &m, None);
        let line = stored(Some("parts"), &l, None);
        let out = recalculate_one(&f, &master, &[line], &Targets::default(), TODAY).unwrap();
        assert!(out.has_errors);
        // 主表沒變，但明細有新的錯誤 → 兩筆都寫入
        assert_eq!(out.writes.len(), 2);
        assert_eq!(out.writes[0].id, master.id);
        assert_eq!(out.writes[1].id, line.id);
        assert!(out.writes[1].formula_errors.is_some());
    }

    #[test]
    fn data_must_be_object() {
        let m = json!([1]);
        let master = stored(None, &m, None);
        assert!(matches!(
            recalculate_one(&formulas(), &master, &[], &Targets::default(), TODAY),
            Err(CoreError::Internal(_))
        ));
    }
}

//! 存檔時計算公式欄位（docs/architecture.md §7.6、§8.5 `custom_records`）。
//!
//! 計算本身在 [`hammer_formula::record`]（桌面端的即時預覽使用同一套程式，§3.3），本模組再匯出，
//! 並負責讀取資料庫：
//!
//! - [`load_targets`]：讀取公式的關聯引用 `{relation.field}` 所指資料**目前**的值（含已軟刪除的資料）
//!   與目標資料類別的欄位型態
//! - [`today_of`]：「今天」（`TODAY()`）為存檔時間在 Asia/Taipei 的日期（D13）
//!
//! 新增與修改資料時（[`crate::records`]）在寫入的 transaction 內計算；失敗的欄位原因存入
//! `custom_records.formula_errors`，不中斷存檔。

use std::collections::{BTreeMap, BTreeSet};

use hammer_db::record as record_db;
use hammer_db::schema as schema_db;
use hammer_db::sqlx::PgConnection;
pub use hammer_formula::record::{
    FormulaErrors, Formulas, MSG_NO_RESULT_TYPE, Targets, errors_from_json, errors_to_json,
    value_type,
};
use hammer_shared::Id;
use hammer_shared::time::{Date, TAIPEI_OFFSET};
use serde_json::Value;
use time::OffsetDateTime;

use crate::CoreError;
use crate::schema::field_from_row;

/// 存檔時間在 Asia/Taipei 的日期，即公式的 `TODAY()`（D13）。
pub fn today_of(now: OffsetDateTime) -> Date {
    now.to_offset(TAIPEI_OFFSET).date()
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

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::{date, datetime};

    #[test]
    fn today_uses_taipei_date() {
        // UTC 11/15 16:00 = 台北 11/16 00:00
        assert_eq!(
            today_of(datetime!(2026-11-15 16:00 UTC)),
            date!(2026 - 11 - 16)
        );
        assert_eq!(
            today_of(datetime!(2026-11-15 15:59 UTC)),
            date!(2026 - 11 - 15)
        );
    }
}

//! 產生資料標題時存取資料庫的部分（docs/architecture.md §8.5）。
//!
//! 標題範本的關聯引用（`{customer.short_name}`）需要目標資料類別的欄位定義與目標資料。
//! [`Titles::load`] 讀取範本引用的每個目標資料類別的欄位定義（每個目標資料類別固定 2 次查詢）；
//! [`Titles::related`] 依目標資料類別分組，以 id 批次讀取一批資料需要的目標資料（每個目標資料類別
//! 1 次查詢）。查詢次數只與範本引用的目標資料類別數量有關，不隨資料筆數增加。
//!
//! 格式化等純邏輯在 [`crate::title`]，有單元測試；存取資料庫的部分由
//! `crates/server/tests/title_relation_api.rs` 的整合測試涵蓋。

use std::collections::{BTreeMap, HashMap};

use hammer_db::record as record_db;
use hammer_db::record::{RecordRow, RelatedRecordRow};
use hammer_db::schema as schema_db;
use hammer_db::sqlx::PgConnection;
use hammer_db::system_record::{self as system_db, SystemTable};
use hammer_shared::Id;
use hammer_shared::record::Record;
use hammer_shared::schema::{EntityDefinition, FieldDefinition};
use serde_json::{Map, Value};

use crate::CoreError;
use crate::records::{fields_and_table, record_from_row};
use crate::schema::entity_from_row;
use crate::title::{RelatedRecord, RelatedRecords, TitleRenderer};

/// 一個資料類別的標題產生器，含關聯引用的目標資料類別。
pub(crate) struct Titles<'a> {
    renderer: TitleRenderer<'a>,
    /// 目標資料類別的資料存放位置：`None` 為 `custom_records`，`Some` 為系統資料表。
    stores: HashMap<Id, Option<SystemTable>>,
}

impl<'a> Titles<'a> {
    /// 依資料類別的範本與欄位定義（含已停用）建立，並讀取範本引用的目標資料類別的欄位定義。
    /// 目標資料類別不存在（已刪除）時略過，該關聯引用為空字串。
    pub(crate) async fn load(
        conn: &mut PgConnection,
        entity: &EntityDefinition,
        fields: &'a [FieldDefinition],
    ) -> Result<Self, CoreError> {
        let mut renderer = TitleRenderer::new(entity.title_template.as_deref(), fields);
        let mut stores = HashMap::new();
        for target_id in renderer.target_entities() {
            let Some(row) = schema_db::find_entity_by_id(&mut *conn, target_id).await? else {
                continue;
            };
            let table_name = row.table_name.clone();
            let target = entity_from_row(row)?;
            let (target_fields, table) = fields_and_table(conn, &target, table_name).await?;
            renderer.set_target_fields(target_id, target_fields);
            stores.insert(target_id, table);
        }
        Ok(Self { renderer, stores })
    }

    /// 讀取 `datas`（一批資料的 `data`）產生標題需要的目標資料（含已軟刪除的），
    /// 每個目標資料類別一次查詢。
    pub(crate) async fn related<'d>(
        &self,
        conn: &mut PgConnection,
        datas: impl IntoIterator<Item = &'d Map<String, Value>>,
    ) -> Result<RelatedRecords, CoreError> {
        let mut by_entity: BTreeMap<Id, Vec<Id>> = BTreeMap::new();
        for data in datas {
            for (entity_id, id) in self.renderer.related_ids(data) {
                by_entity.entry(entity_id).or_default().push(id);
            }
        }
        let mut related = RelatedRecords::new();
        for (entity_id, mut ids) in by_entity {
            ids.sort();
            ids.dedup();
            let rows = match self.stores.get(&entity_id) {
                Some(Some(table)) => {
                    system_db::find_records_by_ids(&mut *conn, table, &ids).await?
                }
                Some(None) => record_db::find_records_by_ids(&mut *conn, entity_id, &ids).await?,
                None => continue,
            };
            for RelatedRecordRow {
                id,
                record_no,
                data,
                ..
            } in rows
            {
                if let Value::Object(data) = data {
                    related.insert((entity_id, id), RelatedRecord { record_no, data });
                }
            }
        }
        Ok(related)
    }

    /// 產生一筆資料的標題。`related` 由 [`Self::related`] 讀取。
    pub(crate) fn render(
        &self,
        id: Id,
        record_no: Option<&str>,
        data: &Map<String, Value>,
        related: &RelatedRecords,
    ) -> String {
        self.renderer.render(id, record_no, data, related)
    }

    /// 資料列轉成 API 回應（[`record_from_row`]），批次讀取標題需要的目標資料。
    pub(crate) async fn records(
        &self,
        conn: &mut PgConnection,
        rows: Vec<RecordRow>,
    ) -> Result<Vec<Record>, CoreError> {
        let related = self
            .related(conn, rows.iter().filter_map(|r| r.data.as_object()))
            .await?;
        rows.into_iter()
            .map(|row| record_from_row(row, &self.renderer, &related))
            .collect()
    }

    /// 同 [`Self::records`]，一筆。
    pub(crate) async fn record(
        &self,
        conn: &mut PgConnection,
        row: RecordRow,
    ) -> Result<Record, CoreError> {
        let [record] = self
            .records(conn, vec![row])
            .await?
            .try_into()
            .map_err(|_| CoreError::Internal("資料筆數不符".into()))?;
        Ok(record)
    }
}

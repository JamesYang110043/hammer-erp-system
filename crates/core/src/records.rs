//! 通用資料 API 的資料（docs/architecture.md §6.1、§7.1、§7.5、§8.5 `custom_records`、§8.6、§9.4、§10）。
//!
//! - 讀取（需要 `record.read`）：[`list_records`]、[`get_record`]、[`lookup_records`]（選擇器搜尋）
//! - 寫入（需要 `record.write`）：[`create_record`]、[`update_record`]、[`delete_record`]
//!
//! 只處理有效（啟用、未刪除）、不是明細子表的資料類別，其他一律回傳 [`CoreError::NotFound`]。
//! `data` 依欄位定義驗證（[`crate::record_data`]）。
//!
//! 資料依資料類別的 `storage` 分流（[`Store`]）：
//!
//! - `custom_records`：所有欄位存在 `custom_records.data`（`hammer_db::record`）
//! - `table`（系統資料類別，§7.1）：系統欄位（`storage = column`）存在專屬資料表的實體欄位，其他欄位存在
//!   `custom_data`（`hammer_db::system_record`）。request 與回應的 `data` 同時包含兩者，驗證、篩選與
//!   排序、樂觀鎖定、操作紀錄、標題都與自訂資料類別相同；沒有單號。系統明細子表（聯絡人）的明細存在
//!   明細子表的專屬資料表（[`LineStore`]），父資料欄位為 `<主表代碼>_id`（例如 `customer_id`），規則與
//!   自訂明細子表相同
//!
//! 寫入都在單一 transaction 內完成，並寫入一筆 `audit_logs`：`entity_key` 為資料類別代碼，
//! `before_data` / `after_data` 只包含有變動的欄位（[`AuditDiff`]）。修改沒有任何變動時不寫入。
//!
//! 系統資料類別另外套用基本資料的業務規則（[`crate::master_data`]）：系統欄位的固定限制、廠商的預設
//! 幣別、商品分類不可循環，以及單位、分類使用中時不能刪除。
//!
//! 違反「不可重複」欄位的唯一索引（§7.5）時，找出與其他資料重複的欄位，回傳
//! [`CoreError::InvalidFields`]（`422 VALIDATION`）。
//!
//! 資料類別有設定單號規則時，新增資料在同一個 transaction 內取號（[`crate::numbering`]、
//! `hammer_db::number_sequence`）並寫入 `record_no`；單號不可由客戶端指定或修改。
//!
//! 系統單據（`kind = document` 的系統資料類別，§7.1「系統單據」）另外：
//!
//! - 新增時依單號規則取號，寫入主表的 `record_no` 實體欄位；`status` 由伺服器寫入 `draft`
//! - request 的 `data` 帶了系統維護的欄位（`status`、金額合計：唯讀的系統欄位）一律忽略
//! - 只有 `draft` 可以修改、刪除，其他狀態 → [`CoreError::InvalidStatus`]（檢查順序：權限 → 資料存在 →
//!   `version` → 狀態 → 內容）
//! - 回應的 [`Record::document`] 為過帳、作廢資訊；過帳、作廢在 [`crate::documents`]
//!
//! 列表的 query string 格式見 [`hammer_shared::record`]；欄位與運算子的檢查在 [`plan_list`]。
//!
//! 回應的 [`Record::title`] 依資料類別的 `title_template` 產生（[`crate::title`]）。
//!
//! 關聯欄位（`relation`）：寫入時檢查有變動的關聯所指的資料存在且未刪除，回應填入
//! [`Record::relations`]（[`crate::relations`]）。篩選只支援 `eq`、`empty`，不能排序。
//!
//! 附件欄位（`file`）：寫入時在同一個 transaction 內檢查並綁定有變動的附件，被換掉或清空的舊附件
//! 標記為刪除（[`crate::attachments`]）；軟刪除資料時附件保留。篩選只支援 `empty`，不能排序。
//!
//! 明細子表（`kind = detail`）：不能透過這裡的函式直接存取（[`CoreError::NotFound`]），明細與主表
//! 一起存檔（request 的 `lines`，檢查在 [`crate::record_lines`]）。修改時有帶到的明細子表整組取代；
//! 明細的樂觀鎖定由主表的 `version` 負責，明細有變動時主表的 `version` 也遞增；軟刪除主表時明細一起
//! 軟刪除。明細的變動記在主表那筆操作紀錄的 `lines.<明細子表代碼>`。單筆查詢與寫入的回應包含明細
//! （[`Record::lines`]），列表不包含。
//!
//! 公式欄位（`formula`）：新增與修改時在同一個 transaction 內計算（[`crate::formulas`]），先算每一列
//! 明細、再算主表；修改時所有公式都重算，有公式時 request 沒帶到的明細子表也一起重算（主表的子表彙總
//! 需要所有明細）。request 帶的公式欄位值一律忽略。計算失敗的欄位在 `data` 中沒有值，原因寫入
//! `formula_errors`（[`Record::formula_errors`]），不中斷存檔。公式的值變動記在操作紀錄的 diff。
//! 篩選與排序依 `formula_result_type`。

use hammer_db::DbError;
use hammer_db::document as document_db;
use hammer_db::ensure_version_matched;
use hammer_db::number_sequence;
use hammer_db::record::{
    self as db, CompareOp, JsonCast, LineRow, NewRecord, OrderTarget, RECORD_NO_UNIQUE,
    RecordCondition, RecordListFilter, RecordOrder, RecordRow,
};
use hammer_db::schema as schema_db;
use hammer_db::sqlx::postgres::PgQueryResult;
use hammer_db::sqlx::{PgConnection, PgPool, PgTransaction};
use hammer_db::system_record::{
    self as system_db, ColumnType, NewSystemRecord, SystemColumn, SystemDetailTable,
    SystemLineUpdate, SystemRecordUpdate, SystemTable,
};
use hammer_shared::document::{DocumentInfo, DocumentStatus, STATUS_FIELD};
use hammer_shared::pagination::{Page, PageRequest};
use hammer_shared::record::{
    CreateRecordRequest, FilterOp, LineInput, LookupItem, LookupQuery, QueryParamError, RECORD_NO,
    Record, RecordFilter, RecordListQuery, UpdateRecordRequest, query_param, title_matches,
};
use hammer_shared::schema::{
    DataType, EntityDefinition, EntityKind, EntityStorage, FieldDefinition, FieldStorage,
    parse_value, to_json,
};
use hammer_shared::{Id, audit as action, new_id, permission};
use serde_json::{Map, Value};
use std::borrow::Cow;
use std::collections::BTreeMap;
use time::OffsetDateTime;

use crate::CoreError;
use crate::attachments::{FileChanges, apply_file_changes, attach_files, prepare_file_changes};
use crate::audit::{self, AuditDiff, AuditEntry};
use crate::auth::AuthContext;
use crate::formulas::{
    FormulaErrors, Formulas, errors_from_json, errors_to_json, load_targets, today_of, value_type,
};
use crate::master_data;
use crate::numbering::{NumberingPattern, numbering_date};
use crate::record_data::{FieldError, WriteMode, normalize_record_data};
use crate::record_lines::{
    LineError, LineFiles, LineTarget, audit_key, audit_value, duplicate_attachments,
    duplicate_line_values, match_lines, unknown_details, validation_error,
};
use crate::record_titles::Titles;
use crate::relations::{attach_relations, check_relations};
use crate::schema::{ENTITY_NOT_FOUND, ensure_version, entity_from_row, field_from_row};
use crate::title::{RelatedRecords, TitleRenderer};

/// 資料不存在或已刪除時的錯誤訊息。
pub const RECORD_NOT_FOUND: &str = "找不到此資料";
/// 「不可重複」欄位的值已被其他資料使用時的錯誤訊息。
pub const VALUE_TAKEN: &str = "此值已被其他資料使用";
/// 違反唯一索引，但找不到重複的欄位時（例如另一筆資料剛好被刪除）的錯誤訊息。
pub const DUPLICATE_RECORD: &str = "資料與其他資料重複，請重新確認後再送出";
/// 產生的單號與既有的單號重複時（修改單號規則後才可能發生）的錯誤訊息。
pub const RECORD_NO_TAKEN: &str = "產生的單號已被其他資料使用，請調整此資料類別的單號規則";

// ---------------------------------------------------------------------------
// 查詢
// ---------------------------------------------------------------------------

/// 依 query string 分頁查詢資料。需要 `record.read`。
///
/// `pairs` 為 URL 解碼後的 `(名稱, 值)`，格式見 [`hammer_shared::record`]。
///
/// - `page`、`page_size` 不是整數 → [`CoreError::BadRequest`]
/// - 篩選、排序、分頁參數不合法 → [`CoreError::InvalidParam`]（`details.field` 為參數名稱）
pub async fn list_records(
    pool: &PgPool,
    ctx: &AuthContext,
    entity_key: &str,
    pairs: &[(String, String)],
) -> Result<Page<Record>, CoreError> {
    ctx.require(permission::RECORD_READ)?;
    let mut conn = pool.acquire().await.map_err(DbError::from)?;
    let (entity, fields, store) = load_entity(&mut conn, entity_key).await?;

    let query = RecordListQuery::from_pairs(pairs).map_err(|err| match err {
        QueryParamError::NotInteger { .. } => CoreError::BadRequest(err.message()),
        QueryParamError::Invalid { param, message } => CoreError::InvalidParam { param, message },
    })?;
    let (filter, page) = plan_list(entity.id, &fields, &query)?;

    let total = store.count(&mut conn, &filter).await?;
    let rows = store
        .list(&mut conn, &filter, page.limit(), page.offset())
        .await?;
    let titles = Titles::load(&mut conn, &entity, &fields).await?;
    let mut items = titles.records(&mut conn, rows).await?;
    attach_relations(&mut conn, &fields, &mut items).await?;
    attach_files(&mut conn, &fields, &mut items).await?;
    attach_documents(&mut conn, &store, &mut items).await?;
    let total = u32::try_from(total).unwrap_or(u32::MAX);
    Ok(Page::new(items, total, page))
}

/// 取得一筆資料。需要 `record.read`。
pub async fn get_record(
    pool: &PgPool,
    ctx: &AuthContext,
    entity_key: &str,
    id: Id,
) -> Result<Record, CoreError> {
    ctx.require(permission::RECORD_READ)?;
    let mut conn = pool.acquire().await.map_err(DbError::from)?;
    let (entity, fields, store) = load_entity(&mut conn, entity_key).await?;
    let row = store
        .find(&mut conn, entity.id, id)
        .await?
        .ok_or_else(record_not_found)?;
    let details = schema_db::list_detail_entities(&mut *conn, entity.id).await?;
    let details = load_details(&mut conn, &entity, details).await?;
    let record = Titles::load(&mut conn, &entity, &fields)
        .await?
        .record(&mut conn, row)
        .await?;
    let mut record = with_relations(&mut conn, &fields, &store, record).await?;
    record.lines = Some(fetch_lines(&mut conn, &details, id).await?);
    Ok(record)
}

/// 選擇器搜尋：依關鍵字比對資料標題，回傳未刪除的資料的 id 與標題。需要 `record.read`。
/// 系統資料類別另外排除 `is_active = false` 的資料（§7.1）。
///
/// 標題在讀取時產生、不存入資料庫（§8.5），所以依 id 由新到舊分批讀取資料、產生標題後比對，
/// 直到找到 `limit` 筆或讀完為止。query string 格式見 [`LookupQuery`]。
///
/// - `limit` 不是整數 → [`CoreError::BadRequest`]；小於 1 → [`CoreError::InvalidParam`]
pub async fn lookup_records(
    pool: &PgPool,
    ctx: &AuthContext,
    entity_key: &str,
    pairs: &[(String, String)],
) -> Result<Vec<LookupItem>, CoreError> {
    /// 每批讀取的筆數。
    const BATCH: i64 = 200;

    ctx.require(permission::RECORD_READ)?;
    let mut conn = pool.acquire().await.map_err(DbError::from)?;
    let (entity, fields, store) = load_entity(&mut conn, entity_key).await?;
    let query = LookupQuery::from_pairs(pairs).map_err(|err| match err {
        QueryParamError::NotInteger { .. } => CoreError::BadRequest(err.message()),
        QueryParamError::Invalid { param, message } => CoreError::InvalidParam { param, message },
    })?;
    let limit = query.effective_limit() as usize;
    let terms = query.terms();
    let titles = Titles::load(&mut conn, &entity, &fields).await?;

    let mut items = Vec::new();
    let mut before = None;
    loop {
        let rows = store
            .list_before(&mut conn, entity.id, before, BATCH)
            .await?;
        let exhausted = (rows.len() as i64) < BATCH;
        before = rows.last().map(|r| r.id);
        // 關聯引用的目標資料每批一次讀取
        let related = titles
            .related(&mut conn, rows.iter().filter_map(|r| r.data.as_object()))
            .await?;
        for row in rows {
            let Some(data) = row.data.as_object() else {
                continue;
            };
            let title = titles.render(row.id, row.record_no.as_deref(), data, &related);
            if title_matches(&title, &terms) {
                items.push(LookupItem { id: row.id, title });
                if items.len() >= limit {
                    return Ok(items);
                }
            }
        }
        if exhausted {
            return Ok(items);
        }
    }
}

// ---------------------------------------------------------------------------
// 寫入
// ---------------------------------------------------------------------------

/// 新增資料。需要 `record.write`。
///
/// 資料類別有設定單號規則時，依 `now` 的 Asia/Taipei 日期取號並寫入 `record_no`。
///
/// 明細（`req.lines`）與主表一起建立，順序為陣列中的位置。
///
/// - `data` 不符合欄位定義、違反「不可重複」→ [`CoreError::InvalidFields`]
/// - 明細不合法 → [`CoreError::InvalidLines`]（含主表的欄位錯誤）
/// - 產生的單號與既有的單號重複 → [`CoreError::Validation`]（[`RECORD_NO_TAKEN`]）
pub async fn create_record(
    pool: &PgPool,
    ctx: &AuthContext,
    entity_key: &str,
    req: &CreateRecordRequest,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<Record, CoreError> {
    ctx.require(permission::RECORD_WRITE)?;

    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let (entity, fields, store) = load_target_for_write(&mut tx, entity_key).await?;
    let details = load_details_for_write(&mut tx, &entity).await?;
    let formulas = formulas_of(&fields, &details);
    let input = document_input(&store, &fields, &req.data);
    let master = normalize_record_data(&fields, &input, WriteMode::Create);
    // 系統單據新增時一律是草稿（§7.1「系統單據」）
    let master = master.map(|mut data| {
        if store.document_table().is_some() {
            data.insert(
                STATUS_FIELD.to_owned(),
                Value::String(DocumentStatus::Draft.as_str().to_owned()),
            );
        }
        data
    });
    let master = system_rules(&store, &entity.key, master, None);
    let mut line_errors = Vec::new();
    let mut lines =
        prepare_lines(&mut tx, &details, &req.lines, None, false, &mut line_errors).await?;
    line_errors.extend(system_line_rules(&lines));
    let mut data = validation_error(master, line_errors)?;
    check_relations(&mut tx, &fields, &data, None).await?;
    if let Store::Table(_) = store {
        master_data::check_references(&mut tx, &entity.key, None, &data, None).await?;
    }
    let files = prepare_file_changes(&mut tx, &fields, &data, None).await?;
    ensure_unique_attachments(&files, &lines)?;
    let formula_errors = compute_formulas(&mut tx, &formulas, &mut data, &mut lines, now).await?;
    let formula_errors = errors_to_json(&formula_errors);
    let data = Value::Object(data);

    // 驗證通過後才取號：流水號的列鎖定到 transaction 結束，越晚取號鎖定的時間越短
    let record_no = match &store {
        Store::CustomRecords => next_record_no(&mut tx, &entity, now).await?,
        // 系統單據一律有單號規則（不可清空，§7.1「系統單據」）
        Store::Table(table) if table.is_document() => Some(
            next_record_no(&mut tx, &entity, now)
                .await?
                .ok_or_else(|| {
                    CoreError::Internal(format!("系統單據 {} 沒有設定單號規則", entity.key))
                })?,
        ),
        // 主檔類的系統資料類別沒有單號（§7.1）
        Store::Table(_) => None,
    };
    let id = new_id();
    let inserted = store
        .insert(
            &mut tx,
            &NewRecord {
                id,
                entity_id: entity.id,
                record_no: record_no.as_deref(),
                parent_record_id: None,
                sort_order: 0,
                data: &data,
                formula_errors: formula_errors.as_ref(),
                created_by: ctx.user_id,
                created_at: now,
            },
        )
        .await;
    if let Err(err) = inserted {
        return Err(write_error(pool, tx, err, &store, entity.id, id, &fields, &data).await);
    }
    if let Err(err) = write_lines(&mut tx, ctx, id, &lines, now).await {
        return Err(line_write_error(pool, tx, err, id, &lines).await);
    }
    apply_file_changes(&mut tx, ctx, &entity.key, id, &files, client_info, now).await?;
    apply_line_files(&mut tx, ctx, &lines, client_info, now).await?;

    // 操作紀錄包含產生的單號（`record_no` 是保留的代碼，不會與欄位衝突）與明細
    let mut after = data;
    if let Value::Object(map) = &mut after {
        if let Some(no) = &record_no {
            map.insert(RECORD_NO.to_owned(), Value::String(no.clone()));
        }
        for detail in lines.iter().filter(|d| !d.lines.is_empty()) {
            map.insert(audit_key(&detail.detail.entity.key), detail.after_audit());
        }
    }
    record_audit(
        &mut tx,
        ctx,
        action::CREATE,
        &entity.key,
        id,
        AuditDiff::between(None, Some(&after))?,
        client_info,
        now,
    )
    .await?;
    let record = fetch_record(&mut tx, &entity, &fields, &store, &details, id).await?;
    commit(tx).await?;

    tracing::info!(actor = %ctx.user_id, entity = %entity.key, record_id = %id, "已新增資料");
    Ok(record)
}

/// 修改資料：只合併有帶到的欄位，`null` 代表清空。需要 `record.write`。
///
/// 有帶到的明細子表（`req.lines`）整組取代：有 `id` 的更新、沒有 `id` 的新增、沒列出的軟刪除；
/// 沒帶到的明細子表不變。明細有任何變動（含順序）時主表的 `version` 也遞增。
///
/// - 資料不存在或已刪除 → [`CoreError::NotFound`]
/// - `version` 不符 → `Conflict`（§9.4）
/// - `data` 不符合欄位定義、違反「不可重複」→ [`CoreError::InvalidFields`]
/// - 明細不合法 → [`CoreError::InvalidLines`]（含主表的欄位錯誤）
/// - 主表與明細都沒有任何變動時不寫入，回傳目前的資料（`version` 不變）
pub async fn update_record(
    pool: &PgPool,
    ctx: &AuthContext,
    entity_key: &str,
    id: Id,
    req: &UpdateRecordRequest,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<Record, CoreError> {
    ctx.require(permission::RECORD_WRITE)?;

    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let (entity, fields, store) = load_target_for_write(&mut tx, entity_key).await?;
    let current = lock_record(&mut tx, &store, entity.id, id).await?;
    ensure_version(current.version, req.version)?;
    ensure_draft(&store, &current, "修改")?;

    let details = load_details_for_write(&mut tx, &entity).await?;
    let formulas = formulas_of(&fields, &details);
    let existing = data_object(&current.data)?;
    let input = document_input(&store, &fields, &req.data);
    let master = normalize_record_data(&fields, &input, WriteMode::Update { existing });
    let master = system_rules(&store, &entity.key, master, Some(existing));
    let mut line_errors = Vec::new();
    // 有公式時所有明細都要重算（主表的子表彙總也需要沒帶到的明細）
    let recompute_all = !formulas.is_empty();
    let mut lines = prepare_lines(
        &mut tx,
        &details,
        &req.lines,
        Some(id),
        recompute_all,
        &mut line_errors,
    )
    .await?;
    line_errors.extend(system_line_rules(&lines));
    let mut data = validation_error(master, line_errors)?;
    let formula_errors = compute_formulas(&mut tx, &formulas, &mut data, &mut lines, now).await?;
    let formula_errors = errors_to_json(&formula_errors);
    if data == *existing
        && formula_errors == current.formula_errors
        && lines.iter().all(|d| !d.changed())
    {
        let record = Titles::load(&mut tx, &entity, &fields)
            .await?
            .record(&mut tx, current)
            .await?;
        let mut record = with_relations(&mut tx, &fields, &store, record).await?;
        record.lines = Some(fetch_lines(&mut tx, &details, id).await?);
        commit(tx).await?;
        return Ok(record);
    }
    // 只檢查有變動的關聯：目標之後被刪除時，仍可修改其他欄位
    check_relations(&mut tx, &fields, &data, Some(existing)).await?;
    if let Store::Table(_) = store {
        master_data::check_references(&mut tx, &entity.key, Some(id), &data, Some(existing))
            .await?;
    }
    let files = prepare_file_changes(&mut tx, &fields, &data, Some(existing)).await?;
    ensure_unique_attachments(&files, &lines)?;
    let data = Value::Object(data);

    // 只有明細變動時 data 不變，但 version 仍遞增
    let updated = store
        .update(
            &mut tx,
            id,
            req.version,
            &data,
            formula_errors.as_ref(),
            ctx.user_id,
        )
        .await;
    match updated {
        Ok(result) => {
            ensure_version_matched(result)?;
        }
        Err(err) => {
            return Err(write_error(pool, tx, err, &store, entity.id, id, &fields, &data).await);
        }
    }
    if let Err(err) = write_lines(&mut tx, ctx, id, &lines, now).await {
        return Err(line_write_error(pool, tx, err, id, &lines).await);
    }
    apply_file_changes(&mut tx, ctx, &entity.key, id, &files, client_info, now).await?;
    apply_line_files(&mut tx, ctx, &lines, client_info, now).await?;

    let mut before = current.data.clone();
    let mut after = data;
    if let (Value::Object(b), Value::Object(a)) = (&mut before, &mut after) {
        for detail in &lines {
            let key = audit_key(&detail.detail.entity.key);
            b.insert(key.clone(), detail.before_audit());
            a.insert(key, detail.after_audit());
        }
    }
    record_audit(
        &mut tx,
        ctx,
        action::UPDATE,
        &entity.key,
        id,
        AuditDiff::between(Some(&before), Some(&after))?,
        client_info,
        now,
    )
    .await?;
    let record = fetch_record(&mut tx, &entity, &fields, &store, &details, id).await?;
    commit(tx).await?;

    tracing::info!(actor = %ctx.user_id, entity = %entity.key, record_id = %id, "已修改資料");
    Ok(record)
}

/// 軟刪除資料，明細一起軟刪除（附件保留）。需要 `record.write`。
///
/// - 資料不存在或已刪除 → [`CoreError::NotFound`]
/// - `version` 不符 → `Conflict`（§9.4）
pub async fn delete_record(
    pool: &PgPool,
    ctx: &AuthContext,
    entity_key: &str,
    id: Id,
    version: i32,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<(), CoreError> {
    ctx.require(permission::RECORD_WRITE)?;

    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let (entity, _, store) = load_target_for_write(&mut tx, entity_key).await?;
    let current = lock_record(&mut tx, &store, entity.id, id).await?;
    ensure_version(current.version, version)?;
    ensure_draft(&store, &current, "刪除")?;
    if let Store::Table(_) = store {
        master_data::check_delete(&mut tx, &entity.key, id).await?;
    }
    // 含已停用的明細子表
    let details = load_details_for_write(&mut tx, &entity).await?;
    let lines = detail_lines(&mut tx, &details, id).await?;

    ensure_version_matched(
        store
            .soft_delete(&mut tx, id, version, now, ctx.user_id)
            .await?,
    )?;
    soft_delete_detail_lines(&mut tx, &details, &lines, id, now, ctx.user_id).await?;

    // 操作紀錄包含所有明細（含已停用的明細子表）
    let mut before = current.data.clone();
    if let Value::Object(map) = &mut before {
        for (detail, rows) in details.iter().zip(&lines) {
            if !rows.is_empty() {
                map.insert(
                    audit_key(&detail.entity.key),
                    audit_value(rows.iter().map(|r| (r.id, &r.data))),
                );
            }
        }
    }
    record_audit(
        &mut tx,
        ctx,
        action::DELETE,
        &entity.key,
        id,
        AuditDiff::between(Some(&before), None)?,
        client_info,
        now,
    )
    .await?;
    commit(tx).await?;

    tracing::info!(actor = %ctx.user_id, entity = %entity.key, record_id = %id, "已刪除資料");
    Ok(())
}

// ---------------------------------------------------------------------------
// 列表條件
// ---------------------------------------------------------------------------

/// 依欄位定義檢查列表的篩選、排序與分頁參數，轉成資料庫的查詢條件。
///
/// - 只能使用有效（`is_active`）且目前支援的欄位（系統欄位與自訂欄位都可以，§10），或代表單號的
///   [`RECORD_NO`]
///   （排序依文字；篩選只支援 `eq`、`contains`）
/// - 運算子必須適用於欄位型態（見 [`FilterOp`]）；值必須符合型態的格式
/// - 同一個欄位的多個 `any` 合併成一個條件
///
/// 錯誤都是 [`CoreError::InvalidParam`]，`param` 為參數名稱。
pub fn plan_list(
    entity_id: Id,
    fields: &[FieldDefinition],
    query: &RecordListQuery,
) -> Result<(RecordListFilter, PageRequest), CoreError> {
    let page =
        PageRequest::new(query.page, query.page_size).map_err(|err| CoreError::InvalidParam {
            param: err.field.to_owned(),
            message: err.message().to_owned(),
        })?;

    let order = query
        .sort
        .as_ref()
        .map(|sort| {
            if sort.field_key == RECORD_NO {
                return Ok(RecordOrder {
                    target: OrderTarget::RecordNo,
                    descending: sort.descending,
                });
            }
            let field = queryable_field(fields, &sort.field_key, query_param::SORT)?;
            let cast = sort_cast(query_type(field)).ok_or_else(|| CoreError::InvalidParam {
                param: query_param::SORT.to_owned(),
                message: "此欄位型態不支援排序".to_owned(),
            })?;
            Ok::<_, CoreError>(RecordOrder {
                target: OrderTarget::Field {
                    field_key: field.field_key.clone(),
                    cast,
                },
                descending: sort.descending,
            })
        })
        .transpose()?;

    let mut conditions: Vec<RecordCondition> = Vec::new();
    for filter in &query.filters {
        let param = filter.param_name();
        if filter.field_key == RECORD_NO {
            conditions.push(record_no_condition(filter).map_err(|message| {
                CoreError::InvalidParam {
                    param: param.clone(),
                    message,
                }
            })?);
            continue;
        }
        let field = queryable_field(fields, &filter.field_key, &param)?;
        let condition =
            filter_condition(field, filter).map_err(|message| CoreError::InvalidParam {
                param: param.clone(),
                message,
            })?;
        // 同一個欄位的 `any` 合併：符合任一個即可
        if let RecordCondition::ContainsAny { field_key, values } = &condition
            && let Some(RecordCondition::ContainsAny { values: existing, .. }) =
                conditions.iter_mut().find(|c| {
                    matches!(c, RecordCondition::ContainsAny { field_key: k, .. } if k == field_key)
                })
        {
            existing.extend(values.iter().cloned());
            continue;
        }
        conditions.push(condition);
    }

    Ok((
        RecordListFilter {
            entity_id,
            conditions,
            order,
        },
        page,
    ))
}

/// 可用於篩選與排序的欄位：有效；`formula` 欄位必須有結果型態。
fn queryable_field<'a>(
    fields: &'a [FieldDefinition],
    field_key: &str,
    param: &str,
) -> Result<&'a FieldDefinition, CoreError> {
    fields
        .iter()
        .find(|f| f.field_key == field_key && f.is_active && value_type(f).is_some())
        .ok_or_else(|| CoreError::InvalidParam {
            param: param.to_owned(),
            message: format!("沒有可查詢的欄位「{field_key}」"),
        })
}

/// 篩選與排序時欄位的型態：`formula` 欄位為它的 `formula_result_type`（[`value_type`]）。
fn query_type(field: &FieldDefinition) -> DataType {
    value_type(field).unwrap_or(field.data_type)
}

/// 比較與排序時的轉型方式；`multi_select` 只能排序不能比較，`relation` 只能比較（`eq`）不能排序。
fn compare_cast(data_type: DataType) -> Option<JsonCast> {
    match data_type {
        DataType::Text | DataType::LongText | DataType::Select | DataType::Relation => {
            Some(JsonCast::Text)
        }
        DataType::Integer | DataType::Decimal | DataType::Money => Some(JsonCast::Numeric),
        DataType::Date => Some(JsonCast::Date),
        DataType::DateTime => Some(JsonCast::Timestamptz),
        DataType::Boolean => Some(JsonCast::Boolean),
        DataType::MultiSelect | DataType::File | DataType::Formula => None,
    }
}

fn sort_cast(data_type: DataType) -> Option<JsonCast> {
    match data_type {
        DataType::MultiSelect => Some(JsonCast::Jsonb),
        DataType::Relation => None,
        other => compare_cast(other),
    }
}

/// 運算子是否適用於欄位型態。
fn op_supported(data_type: DataType, op: FilterOp) -> bool {
    use DataType as T;
    match op {
        FilterOp::Empty => true,
        FilterOp::Eq => compare_cast(data_type).is_some(),
        FilterOp::Contains => matches!(data_type, T::Text | T::LongText),
        FilterOp::Gt | FilterOp::Gte | FilterOp::Lt | FilterOp::Lte => matches!(
            data_type,
            T::Integer | T::Decimal | T::Money | T::Date | T::DateTime
        ),
        FilterOp::Any => data_type == T::MultiSelect,
    }
}

/// 一個篩選條件轉成資料庫條件。錯誤為顯示給使用者的說明。
fn filter_condition(
    field: &FieldDefinition,
    filter: &RecordFilter,
) -> Result<RecordCondition, String> {
    let data_type = query_type(field);
    if !op_supported(data_type, filter.op) {
        return Err(format!("此欄位型態不支援 {} 篩選", filter.op.as_str()));
    }
    let field_key = field.field_key.clone();
    let compare = |op: CompareOp| -> Result<RecordCondition, String> {
        Ok(RecordCondition::Compare {
            field_key: field.field_key.clone(),
            // op_supported 已確認可比較
            cast: compare_cast(data_type).unwrap_or(JsonCast::Text),
            op,
            value: compare_value(data_type, &filter.value)?,
        })
    };
    match filter.op {
        FilterOp::Eq => compare(CompareOp::Eq),
        FilterOp::Gt => compare(CompareOp::Gt),
        FilterOp::Gte => compare(CompareOp::Gte),
        FilterOp::Lt => compare(CompareOp::Lt),
        FilterOp::Lte => compare(CompareOp::Lte),
        FilterOp::Contains => Ok(RecordCondition::Contains {
            field_key,
            text: filter.value.clone(),
        }),
        FilterOp::Empty => {
            let empty = parse_bool(&filter.value)?;
            Ok(RecordCondition::HasValue {
                field_key,
                has_value: !empty,
            })
        }
        FilterOp::Any => Ok(RecordCondition::ContainsAny {
            field_key,
            values: vec![filter.value.clone()],
        }),
    }
}

/// 單號的篩選條件：只支援 `eq`、`contains`。
fn record_no_condition(filter: &RecordFilter) -> Result<RecordCondition, String> {
    match filter.op {
        FilterOp::Eq => Ok(RecordCondition::RecordNoEq(filter.value.clone())),
        FilterOp::Contains => Ok(RecordCondition::RecordNoContains(filter.value.clone())),
        op => Err(format!(
            "單號不支援 {} 篩選，只能使用 eq、contains",
            op.as_str()
        )),
    }
}

/// 檢查比較值的格式，轉成資料庫可以轉型的文字。文字型態原樣使用。
fn compare_value(data_type: DataType, raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    let json = match data_type {
        DataType::Text | DataType::LongText | DataType::Select => return Ok(raw.to_owned()),
        DataType::Boolean => return parse_bool(trimmed).map(|b| b.to_string()),
        DataType::Integer => Value::from(
            trimmed
                .parse::<i64>()
                .map_err(|_| "必須是整數".to_owned())?,
        ),
        _ => Value::String(trimmed.to_owned()),
    };
    match parse_value(data_type, &json) {
        Ok(Some(value)) => match to_json(&value) {
            Value::String(s) => Ok(s),
            other => Ok(other.to_string()),
        },
        Ok(None) => Err("篩選值不可為空".to_owned()),
        Err(err) => Err(err.message().to_owned()),
    }
}

fn parse_bool(raw: &str) -> Result<bool, String> {
    match raw.trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err("必須是 true 或 false".to_owned()),
    }
}

// ---------------------------------------------------------------------------
// 共用
// ---------------------------------------------------------------------------

/// 載入有效的資料類別、其所有欄位定義（含已停用）與資料的存放位置。
async fn load_entity(
    conn: &mut PgConnection,
    key: &str,
) -> Result<(EntityDefinition, Vec<FieldDefinition>, Store), CoreError> {
    let row = schema_db::find_entity_by_key(&mut *conn, key).await?;
    load_fields_of(conn, row).await
}

/// 同 [`load_entity`]，寫入資料時使用：以 `FOR SHARE` 鎖定資料類別，等待進行中的欄位定義變更
/// （例如變更欄位型態，[`crate::schema::change_field_type`]）完成後才讀取欄位定義，
/// 避免依舊的定義寫入資料。
pub(crate) async fn load_target_for_write(
    tx: &mut PgTransaction<'_>,
    key: &str,
) -> Result<(EntityDefinition, Vec<FieldDefinition>, Store), CoreError> {
    let row = schema_db::find_entity_by_key_for_share(&mut **tx, key).await?;
    load_fields_of(tx, row).await
}

/// 同 [`load_target_for_write`]，供重新計算公式使用（[`crate::recalculate`]）。自訂與系統資料類別都可以。
pub(crate) async fn load_entity_for_write(
    tx: &mut PgTransaction<'_>,
    key: &str,
) -> Result<(EntityDefinition, Vec<FieldDefinition>, Store), CoreError> {
    load_target_for_write(tx, key).await
}

async fn load_fields_of(
    conn: &mut PgConnection,
    row: Option<schema_db::EntityRow>,
) -> Result<(EntityDefinition, Vec<FieldDefinition>, Store), CoreError> {
    let Some(row) = row else {
        return Err(CoreError::NotFound(ENTITY_NOT_FOUND.into()));
    };
    let table_name = row.table_name.clone();
    let entity = entity_from_row(row)?;
    // 明細子表只能透過主表存取
    if !entity.is_active || entity.kind == EntityKind::Detail {
        return Err(CoreError::NotFound(ENTITY_NOT_FOUND.into()));
    }
    let (fields, table) = fields_and_table(conn, &entity, table_name).await?;
    let store = match table {
        None => Store::CustomRecords,
        Some(table) => Store::Table(table),
    };
    Ok((entity, fields, store))
}

/// 資料類別的所有欄位定義（含已停用）；系統資料類別（`storage = table`）另外回傳專屬資料表與系統欄位。
pub(crate) async fn fields_and_table(
    conn: &mut PgConnection,
    entity: &EntityDefinition,
    table_name: Option<String>,
) -> Result<(Vec<FieldDefinition>, Option<SystemTable>), CoreError> {
    let rows = schema_db::list_fields(&mut *conn, entity.id).await?;
    let columns: Vec<(String, Option<String>)> = rows
        .iter()
        .filter(|r| r.storage == FieldStorage::Column.as_str())
        .map(|r| (r.field_key.clone(), r.column_name.clone()))
        .collect();
    let fields = rows
        .into_iter()
        .map(field_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    let table = match entity.storage {
        EntityStorage::CustomRecords => None,
        EntityStorage::Table => {
            let table_name = table_name.ok_or_else(|| {
                CoreError::Internal(format!("系統資料類別 {} 沒有設定 table_name", entity.key))
            })?;
            let table = system_table(entity.id, &table_name, &fields, &columns)?;
            // 系統單據的主表有單號與狀態（§7.1「系統單據」）
            Some(if entity.kind == EntityKind::Document {
                table.document()
            } else {
                table
            })
        }
    };
    Ok((fields, table))
}

/// 系統資料類別的資料表與系統欄位。`columns` 為 `storage = column` 的欄位 `(field_key, column_name)`；
/// 系統欄位的 `column_name` 必須與 `field_key` 相同（§7.1）。名稱不安全或型態不能存成實體欄位時回傳
/// [`CoreError::Internal`]（migration 寫入的定義不正確）。
fn system_table(
    entity_id: Id,
    table_name: &str,
    fields: &[FieldDefinition],
    columns: &[(String, Option<String>)],
) -> Result<SystemTable, CoreError> {
    let mut out = Vec::with_capacity(columns.len());
    for (field_key, column_name) in columns {
        if column_name.as_deref() != Some(field_key.as_str()) {
            return Err(CoreError::Internal(format!(
                "系統欄位 {table_name}.{field_key} 的 column_name 與欄位代碼不同"
            )));
        }
        let field = fields
            .iter()
            .find(|f| f.field_key == *field_key)
            .ok_or_else(|| CoreError::Internal(format!("找不到系統欄位 {field_key}")))?;
        let column_type = column_type(field.data_type).ok_or_else(|| {
            CoreError::Internal(format!(
                "系統欄位 {table_name}.{field_key} 的型態 {} 不能存成實體欄位",
                field.data_type.as_str()
            ))
        })?;
        out.push(SystemColumn {
            name: field_key.clone(),
            column_type,
        });
    }
    SystemTable::new(entity_id, table_name, out).map_err(|e| CoreError::Internal(e.to_string()))
}

/// 系統欄位的 `data_type` 對應的實體欄位型別（§8.6、§8.7）；`multi_select`、`file`、`formula` 不能是
/// 系統欄位。
fn column_type(data_type: DataType) -> Option<ColumnType> {
    match data_type {
        DataType::Text | DataType::LongText | DataType::Select => Some(ColumnType::Text),
        DataType::Integer => Some(ColumnType::Integer),
        DataType::Decimal | DataType::Money => Some(ColumnType::Numeric),
        DataType::Date => Some(ColumnType::Date),
        DataType::DateTime => Some(ColumnType::Timestamptz),
        DataType::Boolean => Some(ColumnType::Boolean),
        DataType::Relation => Some(ColumnType::Uuid),
        DataType::MultiSelect | DataType::File | DataType::Formula => None,
    }
}

/// 資料的存放位置（§7.1）。
pub(crate) enum Store {
    /// 自訂資料類別：`custom_records`。
    CustomRecords,
    /// 系統資料類別：專屬資料表（系統欄位為實體欄位，其他欄位在 `custom_data`）。
    Table(SystemTable),
}

impl Store {
    /// 系統單據的主表（§7.1「系統單據」）；其他資料類別為 `None`。
    pub(crate) fn document_table(&self) -> Option<&SystemTable> {
        match self {
            Self::Table(table) if table.is_document() => Some(table),
            _ => None,
        }
    }

    /// `index` 是否為單號的唯一索引：`custom_records` 為 [`RECORD_NO_UNIQUE`]；系統單據為
    /// `uq_<資料表>_record_no`（migration 的命名慣例）。
    fn is_record_no_index(&self, index: &str) -> bool {
        match self {
            Self::CustomRecords => index == RECORD_NO_UNIQUE,
            Self::Table(table) => {
                table.is_document()
                    && index
                        .strip_prefix("uq_")
                        .and_then(|rest| rest.strip_suffix("_record_no"))
                        == Some(table.name())
            }
        }
    }

    pub(crate) async fn find(
        &self,
        conn: &mut PgConnection,
        entity_id: Id,
        id: Id,
    ) -> Result<Option<RecordRow>, DbError> {
        match self {
            Self::CustomRecords => db::find_record(conn, entity_id, id).await,
            Self::Table(table) => system_db::find_record(conn, table, id).await,
        }
    }

    pub(crate) async fn find_for_update(
        &self,
        conn: &mut PgConnection,
        entity_id: Id,
        id: Id,
    ) -> Result<Option<RecordRow>, DbError> {
        match self {
            Self::CustomRecords => db::find_record_for_update(conn, entity_id, id).await,
            Self::Table(table) => system_db::find_record_for_update(conn, table, id).await,
        }
    }

    async fn list(
        &self,
        conn: &mut PgConnection,
        filter: &RecordListFilter,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<RecordRow>, DbError> {
        match self {
            Self::CustomRecords => db::list_records(conn, filter, limit, offset).await,
            Self::Table(table) => system_db::list_records(conn, table, filter, limit, offset).await,
        }
    }

    async fn count(
        &self,
        conn: &mut PgConnection,
        filter: &RecordListFilter,
    ) -> Result<i64, DbError> {
        match self {
            Self::CustomRecords => db::count_records(conn, filter).await,
            Self::Table(table) => system_db::count_records(conn, table, filter).await,
        }
    }

    async fn list_before(
        &self,
        conn: &mut PgConnection,
        entity_id: Id,
        before: Option<Id>,
        limit: i64,
    ) -> Result<Vec<RecordRow>, DbError> {
        match self {
            Self::CustomRecords => db::list_records_before(conn, entity_id, before, limit).await,
            Self::Table(table) => system_db::list_records_before(conn, table, before, limit).await,
        }
    }

    /// 重新計算時的一批主表資料：未刪除、依 `id` 由小到大、id 大於 `after`，並鎖定（`FOR UPDATE`）。
    pub(crate) async fn list_after_for_update(
        &self,
        conn: &mut PgConnection,
        entity_id: Id,
        after: Option<Id>,
        limit: i64,
    ) -> Result<Vec<RecordRow>, DbError> {
        match self {
            Self::CustomRecords => {
                db::list_records_after_for_update(conn, entity_id, after, limit).await
            }
            Self::Table(table) => {
                system_db::list_records_after_for_update(conn, table, after, limit).await
            }
        }
    }

    /// 新增主表資料。系統資料類別不使用 `parent_record_id`、`sort_order`；只有系統單據使用 `record_no`。
    async fn insert(&self, conn: &mut PgConnection, new: &NewRecord<'_>) -> Result<(), DbError> {
        match self {
            Self::CustomRecords => db::insert_record(conn, new).await,
            Self::Table(table) => {
                let custom_data = custom_data_of(table, new.data);
                system_db::insert_record(
                    conn,
                    table,
                    &NewSystemRecord {
                        id: new.id,
                        record_no: new.record_no.filter(|_| table.is_document()),
                        data: new.data,
                        custom_data: &custom_data,
                        formula_errors: new.formula_errors,
                        created_by: new.created_by,
                        created_at: new.created_at,
                    },
                )
                .await
            }
        }
    }

    async fn update(
        &self,
        conn: &mut PgConnection,
        id: Id,
        version: i32,
        data: &Value,
        formula_errors: Option<&Value>,
        updated_by: Id,
    ) -> Result<PgQueryResult, DbError> {
        match self {
            Self::CustomRecords => {
                db::update_record_data(conn, id, version, data, formula_errors, updated_by).await
            }
            Self::Table(table) => {
                let custom_data = custom_data_of(table, data);
                system_db::update_record(
                    conn,
                    table,
                    &SystemRecordUpdate {
                        id,
                        version,
                        data,
                        custom_data: &custom_data,
                        formula_errors,
                        updated_by,
                    },
                )
                .await
            }
        }
    }

    async fn soft_delete(
        &self,
        conn: &mut PgConnection,
        id: Id,
        version: i32,
        now: OffsetDateTime,
        deleted_by: Id,
    ) -> Result<PgQueryResult, DbError> {
        match self {
            Self::CustomRecords => db::soft_delete_record(conn, id, version, now, deleted_by).await,
            Self::Table(table) => {
                system_db::soft_delete_record(conn, table, id, version, now, deleted_by).await
            }
        }
    }

    async fn value_taken(
        &self,
        pool: &PgPool,
        entity_id: Id,
        exclude_id: Id,
        field_key: &str,
        value: &Value,
    ) -> Result<bool, DbError> {
        match self {
            Self::CustomRecords => {
                db::value_taken(pool, entity_id, exclude_id, field_key, value).await
            }
            Self::Table(table) => {
                system_db::value_taken(pool, table, exclude_id, field_key, value).await
            }
        }
    }
}

/// 系統資料類別在通用欄位驗證之後，再檢查基本資料的業務規則（[`master_data::check_data`]）。
/// 通用欄位驗證已有錯誤時直接回傳，不再檢查。
fn system_rules(
    store: &Store,
    entity_key: &str,
    master: Result<Map<String, Value>, Vec<FieldError>>,
    existing: Option<&Map<String, Value>>,
) -> Result<Map<String, Value>, Vec<FieldError>> {
    let Store::Table(_) = store else {
        return master;
    };
    let mut data = master?;
    let errors = master_data::check_data(entity_key, &mut data, existing);
    if errors.is_empty() {
        Ok(data)
    } else {
        Err(errors)
    }
}

/// 系統單據：去除 request 的 `data` 中系統維護的欄位（唯讀的系統欄位，例如 `status`、金額合計），
/// 這些欄位由伺服器寫入，帶了一律忽略（§7.1「系統單據」）。其他資料類別原樣回傳。
fn document_input<'a>(
    store: &Store,
    fields: &[FieldDefinition],
    input: &'a Map<String, Value>,
) -> Cow<'a, Map<String, Value>> {
    if store.document_table().is_none() {
        return Cow::Borrowed(input);
    }
    let maintained = |key: &str| {
        fields
            .iter()
            .any(|f| f.field_key == key && f.is_system && f.is_readonly)
    };
    if !input.keys().any(|k| maintained(k)) {
        return Cow::Borrowed(input);
    }
    Cow::Owned(
        input
            .iter()
            .filter(|(k, _)| !maintained(k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    )
}

/// 系統單據的狀態（`data.status`）。不是系統單據時為 `None`；值不正確時回傳 [`CoreError::Internal`]。
pub(crate) fn document_status(
    store: &Store,
    row: &RecordRow,
) -> Result<Option<DocumentStatus>, CoreError> {
    if store.document_table().is_none() {
        return Ok(None);
    }
    row.data
        .get(STATUS_FIELD)
        .and_then(Value::as_str)
        .and_then(DocumentStatus::parse)
        .map(Some)
        .ok_or_else(|| CoreError::Internal(format!("單據 {} 的狀態不正確", row.id)))
}

/// 狀態不允許某個操作時的錯誤，例如「已過帳的單據不可修改」。
pub(crate) fn invalid_status(status: DocumentStatus, operation: &str) -> CoreError {
    CoreError::InvalidStatus {
        status,
        message: format!("{}的單據不可{operation}", status.label()),
    }
}

/// 系統單據只有 `draft` 可以修改、刪除（§7.1「系統單據」）。`operation` 為「修改」「刪除」。
fn ensure_draft(store: &Store, row: &RecordRow, operation: &str) -> Result<(), CoreError> {
    match document_status(store, row)? {
        Some(status) if status != DocumentStatus::Draft => Err(invalid_status(status, operation)),
        _ => Ok(()),
    }
}

/// 要寫入 `custom_data` 的部分：`data` 中系統欄位以外的鍵。
fn custom_data_of(table: &SystemTable, data: &Value) -> Value {
    let map = data
        .as_object()
        .map(|m| {
            m.iter()
                .filter(|(k, _)| table.column(k).is_none())
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        })
        .unwrap_or_default();
    Value::Object(map)
}

/// 資料列轉成 API 回應，`title` 由 `titles` 產生（`related` 為關聯引用的目標資料，見
/// [`crate::title`]）；`relations`、`attachments` 為空，由 [`attach_relations`]、[`attach_files`]
/// 填入。`data` 不是 JSON 物件時回傳 [`CoreError::Internal`]。
pub fn record_from_row(
    row: RecordRow,
    titles: &TitleRenderer<'_>,
    related: &RelatedRecords,
) -> Result<Record, CoreError> {
    let Value::Object(data) = row.data else {
        return Err(CoreError::Internal(
            "custom_records.data 不是 JSON 物件".into(),
        ));
    };
    Ok(Record {
        title: titles.render(row.id, row.record_no.as_deref(), &data, related),
        id: row.id,
        record_no: row.record_no,
        data,
        relations: Default::default(),
        attachments: Default::default(),
        formula_errors: errors_from_json(row.formula_errors.as_ref()),
        lines: None,
        document: None,
        created_at: row.created_at,
        created_by: row.created_by,
        updated_at: row.updated_at,
        updated_by: row.updated_by,
        version: row.version,
    })
}

fn data_object(data: &Value) -> Result<&Map<String, Value>, CoreError> {
    data.as_object()
        .ok_or_else(|| CoreError::Internal("資料的 data 不是 JSON 物件".into()))
}

/// 寫入主表資料失敗時的錯誤。違反唯一索引時 rollback 後找出重複的欄位。
#[allow(clippy::too_many_arguments)]
async fn write_error(
    pool: &PgPool,
    tx: PgTransaction<'_>,
    err: DbError,
    store: &Store,
    entity_id: Id,
    record_id: Id,
    fields: &[FieldDefinition],
    data: &Value,
) -> CoreError {
    match err.unique_violation() {
        None => return err.into(),
        Some(index) if store.is_record_no_index(index) => {
            return CoreError::Validation(RECORD_NO_TAKEN.into());
        }
        Some(_) => {}
    }
    // transaction 已中止，不能再查詢
    if let Err(e) = tx.rollback().await {
        return DbError::from(e).into();
    }
    match duplicate_fields(pool, store, entity_id, record_id, fields, data).await {
        Ok(errors) if errors.is_empty() => CoreError::Validation(DUPLICATE_RECORD.into()),
        Ok(errors) => CoreError::InvalidFields(errors),
        Err(e) => e,
    }
}

/// 「不可重複」欄位中，值與其他資料相同的欄位。
async fn duplicate_fields(
    pool: &PgPool,
    store: &Store,
    entity_id: Id,
    record_id: Id,
    fields: &[FieldDefinition],
    data: &Value,
) -> Result<Vec<FieldError>, CoreError> {
    let mut errors = Vec::new();
    for field in fields.iter().filter(|f| f.is_unique) {
        let Some(value) = data.get(&field.field_key) else {
            continue;
        };
        if store
            .value_taken(pool, entity_id, record_id, &field.field_key, value)
            .await?
        {
            errors.push(FieldError {
                field_key: field.field_key.clone(),
                message: VALUE_TAKEN.to_owned(),
            });
        }
    }
    Ok(errors)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn record_audit(
    tx: &mut PgTransaction<'_>,
    ctx: &AuthContext,
    action: &str,
    entity_key: &str,
    record_id: Id,
    diff: AuditDiff,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<(), CoreError> {
    audit::record(
        tx,
        &AuditEntry {
            user_id: Some(ctx.user_id),
            action,
            entity_key: Some(entity_key),
            record_id: Some(record_id),
            diff,
            client_info,
            occurred_at: now,
        },
    )
    .await?;
    Ok(())
}

/// 資料類別有設定單號規則時，取號並產生單號；沒有規則時為 `None`。必須在新增資料的
/// transaction 內呼叫，資料寫入失敗而 rollback 時號碼一併還原。
async fn next_record_no(
    tx: &mut PgTransaction<'_>,
    entity: &EntityDefinition,
    now: OffsetDateTime,
) -> Result<Option<String>, CoreError> {
    let Some(pattern) = entity.numbering_pattern.as_deref() else {
        return Ok(None);
    };
    // 寫入資料類別時已檢查過格式
    let pattern = NumberingPattern::parse(pattern).map_err(|e| {
        CoreError::Internal(format!("資料類別 {} 的單號規則不合法：{e}", entity.key))
    })?;
    let date = numbering_date(now);
    let period_key = pattern.period_key(date);
    let sequence = number_sequence::next_value(&mut **tx, entity.id, &period_key, new_id()).await?;
    Ok(Some(pattern.format(date, sequence)))
}

pub(crate) async fn lock_record(
    tx: &mut PgTransaction<'_>,
    store: &Store,
    entity_id: Id,
    id: Id,
) -> Result<RecordRow, CoreError> {
    store
        .find_for_update(tx, entity_id, id)
        .await?
        .ok_or_else(record_not_found)
}

pub(crate) async fn fetch_record(
    tx: &mut PgTransaction<'_>,
    entity: &EntityDefinition,
    fields: &[FieldDefinition],
    store: &Store,
    details: &[Detail],
    id: Id,
) -> Result<Record, CoreError> {
    let row = store
        .find(tx, entity.id, id)
        .await?
        .ok_or_else(|| CoreError::Internal("寫入後找不到資料".into()))?;
    let record = Titles::load(tx, entity, fields)
        .await?
        .record(tx, row)
        .await?;
    let mut record = with_relations(tx, fields, store, record).await?;
    record.lines = Some(fetch_lines(tx, details, id).await?);
    Ok(record)
}

/// 填入一筆資料的 [`Record::relations`] 與 [`Record::attachments`]。
async fn with_relations(
    conn: &mut PgConnection,
    fields: &[FieldDefinition],
    store: &Store,
    record: Record,
) -> Result<Record, CoreError> {
    let mut records = [record];
    attach_relations(conn, fields, &mut records).await?;
    attach_files(conn, fields, &mut records).await?;
    attach_documents(conn, store, &mut records).await?;
    let [record] = records;
    Ok(record)
}

/// 系統單據：填入 [`Record::document`]（過帳、作廢資訊）。其他資料類別不變（`None`）。
async fn attach_documents(
    conn: &mut PgConnection,
    store: &Store,
    records: &mut [Record],
) -> Result<(), CoreError> {
    let Some(table) = store.document_table() else {
        return Ok(());
    };
    if records.is_empty() {
        return Ok(());
    }
    let ids: Vec<Id> = records.iter().map(|r| r.id).collect();
    let states: BTreeMap<Id, DocumentInfo> = document_db::find_states(&mut *conn, table, &ids)
        .await?
        .into_iter()
        .map(|row| {
            (
                row.id,
                DocumentInfo {
                    posted_at: row.posted_at,
                    posted_by: row.posted_by,
                    voided_at: row.voided_at,
                    voided_by: row.voided_by,
                },
            )
        })
        .collect();
    for record in records {
        record.document = Some(states.get(&record.id).cloned().unwrap_or_default());
    }
    Ok(())
}

fn record_not_found() -> CoreError {
    CoreError::NotFound(RECORD_NOT_FOUND.into())
}

pub(crate) async fn commit(tx: PgTransaction<'_>) -> Result<(), CoreError> {
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 明細
// ---------------------------------------------------------------------------

/// 主表的一個明細子表與其所有欄位定義（含已停用）。
pub(crate) struct Detail {
    pub(crate) entity: EntityDefinition,
    pub(crate) fields: Vec<FieldDefinition>,
    /// 明細的存放位置。
    store: LineStore,
}

/// 明細的存放位置（§7.1、§8.5 `custom_records`「明細子表」）。
enum LineStore {
    /// 自訂明細子表：`custom_records`（`parent_record_id` 指向主表資料）。
    CustomRecords,
    /// 系統明細子表：專屬資料表（例如 `customer_contacts`），父資料欄位為 `<主表代碼>_id`。
    Table(SystemDetailTable),
}

/// 系統明細子表指向主表資料的欄位名稱（§7.1：`customer_id` / `supplier_id`）。
fn parent_column(master_key: &str) -> String {
    format!("{master_key}_id")
}

/// 載入主表 `master` 的明細子表的欄位定義與存放位置。
async fn load_details(
    conn: &mut PgConnection,
    master: &EntityDefinition,
    rows: Vec<schema_db::EntityRow>,
) -> Result<Vec<Detail>, CoreError> {
    let mut details = Vec::with_capacity(rows.len());
    for row in rows {
        let table_name = row.table_name.clone();
        let entity = entity_from_row(row)?;
        let (fields, table) = fields_and_table(conn, &entity, table_name).await?;
        let store = match table {
            None => LineStore::CustomRecords,
            Some(table) => LineStore::Table(
                SystemDetailTable::new(table, parent_column(&master.key))
                    .map_err(|e| CoreError::Internal(e.to_string()))?,
            ),
        };
        details.push(Detail {
            entity,
            fields,
            store,
        });
    }
    Ok(details)
}

/// 寫入時載入主表的明細子表（含已停用），以 `FOR SHARE` 鎖定（同 [`load_entity_for_write`]）。
pub(crate) async fn load_details_for_write(
    tx: &mut PgTransaction<'_>,
    master: &EntityDefinition,
) -> Result<Vec<Detail>, CoreError> {
    let rows = schema_db::list_detail_entities_for_share(&mut **tx, master.id).await?;
    load_details(tx, master, rows).await
}

/// 主表資料 `parent_id` 每個明細子表（`details`，含已停用）的未刪除明細，與 `details` 的順序相同，
/// 每個明細子表內依 `sort_order`、`id` 排序。
pub(crate) async fn detail_lines(
    conn: &mut PgConnection,
    details: &[Detail],
    parent_id: Id,
) -> Result<Vec<Vec<LineRow>>, CoreError> {
    // 自訂明細子表的明細都在 custom_records，一次讀出
    let custom = if details
        .iter()
        .any(|d| matches!(d.store, LineStore::CustomRecords))
    {
        db::list_lines(&mut *conn, parent_id).await?
    } else {
        Vec::new()
    };
    let mut out = Vec::with_capacity(details.len());
    for detail in details {
        let rows = match &detail.store {
            LineStore::CustomRecords => custom
                .iter()
                .filter(|r| r.entity_id == detail.entity.id)
                .cloned()
                .collect(),
            LineStore::Table(table) => system_db::list_lines(&mut *conn, table, parent_id).await?,
        };
        out.push(rows);
    }
    Ok(out)
}

/// 多筆主表資料（`parent_ids`）在每個明細子表（`details`，含已停用）的所有未刪除明細，並鎖定
/// （`FOR UPDATE`）。同一筆主表、同一個明細子表的明細依 `sort_order`、`id` 排序。供重新計算公式使用。
pub(crate) async fn lines_of_parents_for_update(
    tx: &mut PgTransaction<'_>,
    details: &[Detail],
    parent_ids: &[Id],
) -> Result<Vec<LineRow>, CoreError> {
    let mut out = Vec::new();
    // 自訂明細子表的明細都在 custom_records，一次讀出
    if details
        .iter()
        .any(|d| matches!(d.store, LineStore::CustomRecords))
    {
        out.extend(db::list_lines_of_parents_for_update(&mut **tx, parent_ids).await?);
    }
    for detail in details {
        if let LineStore::Table(table) = &detail.store {
            out.extend(
                system_db::list_lines_of_parents_for_update(&mut **tx, table, parent_ids).await?,
            );
        }
    }
    Ok(out)
}

/// 重新計算後要寫入的一筆資料（主表或一列明細）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct FormulaWrite<'a> {
    pub(crate) id: Id,
    /// 明細子表代碼；主表為 `None`。
    pub(crate) detail: Option<&'a str>,
    /// 完整的 `data`（系統資料類別包含系統欄位，寫入時只取自訂欄位）。
    pub(crate) data: &'a Value,
    pub(crate) formula_errors: Option<&'a Value>,
}

/// 一次寫入的多筆公式值（`ids`、`data`、`formula_errors` 依序對應）。
#[derive(Default)]
struct FormulaColumns {
    ids: Vec<Id>,
    data: Vec<Value>,
    errors: Vec<Value>,
}

impl FormulaColumns {
    fn push(&mut self, id: Id, data: Value, errors: Option<&Value>) {
        self.ids.push(id);
        self.data.push(data);
        self.errors.push(errors.cloned().unwrap_or(Value::Null));
    }
}

/// 寫入重新計算的結果：取代 `data`（系統資料表為 `custom_data`）與 `formula_errors`，`version` + 1。
/// 依主表的 `store` 與明細子表（`details`）的存放位置分別寫入；呼叫前必須已鎖定這些列。
pub(crate) async fn write_formula_values(
    tx: &mut PgTransaction<'_>,
    store: &Store,
    details: &[Detail],
    writes: &[FormulaWrite<'_>],
    updated_by: Id,
) -> Result<(), CoreError> {
    let mut custom = FormulaColumns::default();
    let mut tables: Vec<(&SystemTable, FormulaColumns)> = Vec::new();
    for w in writes {
        let table = match w.detail {
            None => match store {
                Store::CustomRecords => None,
                Store::Table(table) => Some(table),
            },
            Some(key) => {
                let detail = details
                    .iter()
                    .find(|d| d.entity.key == key)
                    .ok_or_else(|| CoreError::Internal(format!("找不到明細子表 {key}")))?;
                match &detail.store {
                    LineStore::CustomRecords => None,
                    LineStore::Table(table) => Some(table.table()),
                }
            }
        };
        match table {
            None => custom.push(w.id, w.data.clone(), w.formula_errors),
            Some(table) => {
                let i = match tables.iter().position(|(t, _)| t.name() == table.name()) {
                    Some(i) => i,
                    None => {
                        tables.push((table, FormulaColumns::default()));
                        tables.len() - 1
                    }
                };
                tables[i]
                    .1
                    .push(w.id, custom_data_of(table, w.data), w.formula_errors);
            }
        }
    }
    if !custom.ids.is_empty() {
        db::update_formula_values(
            &mut **tx,
            &custom.ids,
            &custom.data,
            &custom.errors,
            updated_by,
        )
        .await?;
    }
    for (table, cols) in &tables {
        system_db::update_formula_values(
            &mut **tx,
            table,
            &cols.ids,
            &cols.data,
            &cols.errors,
            updated_by,
        )
        .await?;
    }
    Ok(())
}

/// 軟刪除主表資料 `parent_id` 的所有明細（刪除主表時）。`lines` 為 [`detail_lines`] 的結果。
async fn soft_delete_detail_lines(
    tx: &mut PgTransaction<'_>,
    details: &[Detail],
    lines: &[Vec<LineRow>],
    parent_id: Id,
    now: OffsetDateTime,
    deleted_by: Id,
) -> Result<(), CoreError> {
    let mut custom = false;
    for (detail, rows) in details.iter().zip(lines) {
        if rows.is_empty() {
            continue;
        }
        match &detail.store {
            LineStore::CustomRecords => custom = true,
            LineStore::Table(table) => {
                system_db::soft_delete_child_records(&mut **tx, table, parent_id, now, deleted_by)
                    .await?;
            }
        }
    }
    if custom {
        db::soft_delete_child_records(&mut **tx, parent_id, now, deleted_by).await?;
    }
    Ok(())
}

/// 一列明細的寫入方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineAction {
    Insert,
    Update,
    /// 資料與順序都沒有變動。
    Unchanged,
}

/// 既有明細修改前的內容（判斷是否有變動）。
struct PreviousLine {
    sort_order: i32,
    data: Value,
    formula_errors: Option<Value>,
}

/// 一列要寫入的明細（已驗證）。
struct LineWrite {
    id: Id,
    /// 在 request 陣列中的位置，也是寫入的 `sort_order`。
    index: usize,
    sort_order: i32,
    data: Value,
    /// 公式的計算錯誤（[`compute_formulas`] 寫入）。
    formula_errors: Option<Value>,
    files: FileChanges,
    /// 既有的明細；新的明細為 `None`。
    previous: Option<PreviousLine>,
}

impl LineWrite {
    /// 寫入方式：與修改前比較 `data`（含公式的結果）、公式錯誤與順序。
    fn action(&self) -> LineAction {
        match &self.previous {
            None => LineAction::Insert,
            Some(p)
                if p.sort_order == self.sort_order
                    && p.data == self.data
                    && p.formula_errors == self.formula_errors =>
            {
                LineAction::Unchanged
            }
            Some(_) => LineAction::Update,
        }
    }
}

/// 一個明細子表要寫入的內容（request 有帶到的明細子表）。
struct DetailWrite<'a> {
    detail: &'a Detail,
    /// 依 request 的順序。
    lines: Vec<LineWrite>,
    /// 沒有列出、要軟刪除的既有明細。
    removed: Vec<Id>,
    /// 修改前的明細 `(id, data)`，依順序（操作紀錄用）。
    before: Vec<(Id, Value)>,
}

impl DetailWrite<'_> {
    fn changed(&self) -> bool {
        !self.removed.is_empty()
            || self
                .lines
                .iter()
                .any(|l| l.action() != LineAction::Unchanged)
    }

    fn before_audit(&self) -> Value {
        audit_value(self.before.iter().map(|(id, data)| (*id, data)))
    }

    fn after_audit(&self) -> Value {
        audit_value(self.lines.iter().map(|l| (l.id, &l.data)))
    }
}

/// 驗證 request 的明細（`input`），回傳每個有帶到的明細子表要寫入的內容。驗證錯誤加入 `errors`
/// （不中斷，以便一次列出所有錯誤）；有錯誤時回傳的內容不可寫入。
///
/// `parent_id` 為修改的主表資料（鎖定並比對既有明細）；新增時為 `None`。
///
/// `all_details = true` 時（修改有公式的資料，所有明細都要重算），request 沒帶到的有效明細子表也
/// 列入，視為列出所有既有明細且不修改任何欄位。
async fn prepare_lines<'d>(
    tx: &mut PgTransaction<'_>,
    details: &'d [Detail],
    input: &BTreeMap<String, Vec<LineInput>>,
    parent_id: Option<Id>,
    all_details: bool,
    errors: &mut Vec<LineError>,
) -> Result<Vec<DetailWrite<'d>>, CoreError> {
    let active: Vec<&EntityDefinition> = details
        .iter()
        .filter(|d| d.entity.is_active)
        .map(|d| &d.entity)
        .collect();
    errors.extend(unknown_details(input, &active));

    let mut keys: Vec<&str> = input.keys().map(String::as_str).collect();
    if all_details && parent_id.is_some() {
        keys.extend(
            active
                .iter()
                .map(|e| e.key.as_str())
                .filter(|k| !input.contains_key(*k)),
        );
    }

    let mut writes = Vec::new();
    for key in keys {
        let Some(detail) = details
            .iter()
            .find(|d| d.entity.is_active && d.entity.key == key)
        else {
            continue;
        };
        let existing = match parent_id {
            Some(parent_id) => match &detail.store {
                LineStore::CustomRecords => {
                    db::list_lines_for_update(&mut **tx, detail.entity.id, parent_id).await?
                }
                LineStore::Table(table) => {
                    system_db::list_lines_for_update(&mut **tx, table, parent_id).await?
                }
            },
            None => Vec::new(),
        };
        let unchanged: Vec<LineInput>;
        let inputs = match input.get(key) {
            Some(inputs) => inputs,
            None => {
                unchanged = existing
                    .iter()
                    .map(|r| LineInput {
                        id: Some(r.id),
                        data: Map::new(),
                    })
                    .collect();
                &unchanged
            }
        };
        let existing_ids: Vec<Id> = existing.iter().map(|r| r.id).collect();
        let matched = match match_lines(key, inputs, &existing_ids, parent_id.is_none()) {
            Ok(matched) => matched,
            Err(e) => {
                errors.extend(e);
                continue;
            }
        };

        let mut lines = Vec::with_capacity(inputs.len());
        let mut normalized: Vec<Option<Map<String, Value>>> = Vec::with_capacity(inputs.len());
        for (index, (line, target)) in inputs.iter().zip(&matched.targets).enumerate() {
            let current = match *target {
                LineTarget::Existing(i) => Some(&existing[i]),
                LineTarget::New => None,
            };
            let sort_order =
                i32::try_from(index).map_err(|_| CoreError::Validation("明細的列數過多".into()))?;
            match prepare_line(tx, detail, line, current).await? {
                Ok((data, files)) => {
                    normalized.push(Some(data.clone()));
                    lines.push(LineWrite {
                        id: current.map_or_else(new_id, |r| r.id),
                        index,
                        sort_order,
                        data: Value::Object(data),
                        // 沒有公式時保持原樣，由 compute_formulas 取代
                        formula_errors: current.and_then(|r| r.formula_errors.clone()),
                        files,
                        previous: current.map(|r| PreviousLine {
                            sort_order: r.sort_order,
                            data: r.data.clone(),
                            formula_errors: r.formula_errors.clone(),
                        }),
                    });
                }
                Err(field_errors) => {
                    errors.extend(LineError::from_fields(key, index, field_errors));
                    normalized.push(None);
                }
            }
        }
        let refs: Vec<Option<&Map<String, Value>>> =
            normalized.iter().map(Option::as_ref).collect();
        errors.extend(duplicate_line_values(key, &detail.fields, &refs));

        writes.push(DetailWrite {
            detail,
            lines,
            removed: matched.removed.iter().map(|&i| existing[i].id).collect(),
            before: existing.iter().map(|r| (r.id, r.data.clone())).collect(),
        });
    }
    Ok(writes)
}

/// 驗證一列明細：正規化 `data`、檢查有變動的關聯與附件。欄位錯誤放在內層的 `Err`。
async fn prepare_line(
    tx: &mut PgTransaction<'_>,
    detail: &Detail,
    line: &LineInput,
    current: Option<&LineRow>,
) -> Result<Result<(Map<String, Value>, FileChanges), Vec<FieldError>>, CoreError> {
    let existing = current.map(|r| data_object(&r.data)).transpose()?;
    let mode = match existing {
        Some(existing) => WriteMode::Update { existing },
        None => WriteMode::Create,
    };
    let mut data = match normalize_record_data(&detail.fields, &line.data, mode) {
        Ok(data) => data,
        Err(errors) => return Ok(Err(errors)),
    };
    if let LineStore::Table(_) = detail.store {
        let errors = master_data::check_data(&detail.entity.key, &mut data, existing);
        if !errors.is_empty() {
            return Ok(Err(errors));
        }
    }
    match check_relations(tx, &detail.fields, &data, existing).await {
        Ok(()) => {}
        Err(CoreError::InvalidFields(errors)) => return Ok(Err(errors)),
        Err(e) => return Err(e),
    }
    match prepare_file_changes(tx, &detail.fields, &data, existing).await {
        Ok(files) => Ok(Ok((data, files))),
        Err(CoreError::InvalidFields(errors)) => Ok(Err(errors)),
        Err(e) => Err(e),
    }
}

/// 系統明細子表的業務規則（[`master_data::check_lines`]，例如主要聯絡人最多一位）。只檢查通過欄位驗證的
/// 明細；自訂明細子表不檢查。
fn system_line_rules(details: &[DetailWrite<'_>]) -> Vec<LineError> {
    details
        .iter()
        .filter(|d| matches!(d.detail.store, LineStore::Table(_)))
        .flat_map(|d| {
            let rows: Vec<(usize, &Map<String, Value>)> = d
                .lines
                .iter()
                .filter_map(|l| Some((l.index, l.data.as_object()?)))
                .collect();
            master_data::check_lines(&d.detail.entity.key, &rows)
        })
        .collect()
}

/// 主表與有效明細子表的公式欄位（[`Formulas`]）。
pub(crate) fn formulas_of(master_fields: &[FieldDefinition], details: &[Detail]) -> Formulas {
    let details: Vec<(&str, &[FieldDefinition])> = details
        .iter()
        .filter(|d| d.entity.is_active)
        .map(|d| (d.entity.key.as_str(), d.fields.as_slice()))
        .collect();
    Formulas::new(master_fields, &details)
}

/// 計算公式（[`crate::formulas`]）：先算 `lines` 每一列明細（結果與錯誤寫入該列），再算主表
/// `data`，回傳主表的錯誤。`lines` 為要寫入的明細子表；沒有列出的有效明細子表視為沒有明細。
async fn compute_formulas(
    tx: &mut PgTransaction<'_>,
    formulas: &Formulas,
    data: &mut Map<String, Value>,
    lines: &mut [DetailWrite<'_>],
    now: OffsetDateTime,
) -> Result<FormulaErrors, CoreError> {
    let today = today_of(now);
    let wanted = formulas.targets(
        data,
        lines.iter().flat_map(|d| {
            let key = d.detail.entity.key.as_str();
            d.lines
                .iter()
                .filter_map(move |l| Some((key, l.data.as_object()?)))
        }),
    );
    let targets = load_targets(tx, wanted).await?;

    for detail in lines.iter_mut() {
        let key = detail.detail.entity.key.as_str();
        for line in &mut detail.lines {
            if let Value::Object(map) = &mut line.data {
                let errors = formulas.compute_line(key, map, &targets, today);
                line.formula_errors = errors_to_json(&errors);
            }
        }
    }
    let rows: BTreeMap<&str, Vec<&Map<String, Value>>> = lines
        .iter()
        .map(|d| {
            (
                d.detail.entity.key.as_str(),
                d.lines.iter().filter_map(|l| l.data.as_object()).collect(),
            )
        })
        .collect();
    Ok(formulas.compute_master(data, &rows, &targets, today))
}

/// 同一個附件不可同時用在主表與明細（或兩列明細）。
fn ensure_unique_attachments(
    master: &FileChanges,
    details: &[DetailWrite<'_>],
) -> Result<(), CoreError> {
    let lines: Vec<LineFiles<'_>> = details
        .iter()
        .flat_map(|d| {
            d.lines.iter().map(|l| LineFiles {
                detail_key: &d.detail.entity.key,
                index: l.index,
                bind: &l.files.bind,
            })
        })
        .collect();
    let errors = duplicate_attachments(&master.bind, &lines);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(CoreError::InvalidLines {
            fields: Vec::new(),
            lines: errors,
        })
    }
}

/// 寫入明細：先軟刪除沒列出的，再更新、新增（先刪除才不會與「不可重複」的值衝突）。
async fn write_lines(
    tx: &mut PgTransaction<'_>,
    ctx: &AuthContext,
    parent_id: Id,
    details: &[DetailWrite<'_>],
    now: OffsetDateTime,
) -> Result<(), DbError> {
    for detail in details {
        if !detail.removed.is_empty() {
            match &detail.detail.store {
                LineStore::CustomRecords => {
                    db::soft_delete_lines(&mut **tx, &detail.removed, now, ctx.user_id).await?;
                }
                LineStore::Table(table) => {
                    system_db::soft_delete_lines(
                        &mut **tx,
                        table,
                        &detail.removed,
                        now,
                        ctx.user_id,
                    )
                    .await?;
                }
            }
        }
    }
    for detail in details {
        for line in &detail.lines {
            let table = match &detail.detail.store {
                LineStore::CustomRecords => None,
                LineStore::Table(table) => Some(table),
            };
            match (line.action(), table) {
                (LineAction::Unchanged, _) => {}
                (LineAction::Update, None) => {
                    // 已由 list_lines_for_update 鎖定
                    db::update_line(
                        &mut **tx,
                        line.id,
                        line.sort_order,
                        &line.data,
                        line.formula_errors.as_ref(),
                        ctx.user_id,
                    )
                    .await?;
                }
                (LineAction::Update, Some(table)) => {
                    let custom_data = custom_data_of(table.table(), &line.data);
                    system_db::update_line(
                        &mut **tx,
                        table,
                        &SystemLineUpdate {
                            id: line.id,
                            sort_order: line.sort_order,
                            data: &line.data,
                            custom_data: &custom_data,
                            formula_errors: line.formula_errors.as_ref(),
                            updated_by: ctx.user_id,
                        },
                    )
                    .await?;
                }
                (LineAction::Insert, Some(table)) => {
                    let custom_data = custom_data_of(table.table(), &line.data);
                    system_db::insert_line(
                        &mut **tx,
                        table,
                        &NewSystemRecord {
                            id: line.id,
                            record_no: None,
                            data: &line.data,
                            custom_data: &custom_data,
                            formula_errors: line.formula_errors.as_ref(),
                            created_by: ctx.user_id,
                            created_at: now,
                        },
                        parent_id,
                        line.sort_order,
                    )
                    .await?;
                }
                (LineAction::Insert, None) => {
                    db::insert_record(
                        &mut **tx,
                        &NewRecord {
                            id: line.id,
                            entity_id: detail.detail.entity.id,
                            record_no: None,
                            parent_record_id: Some(parent_id),
                            sort_order: line.sort_order,
                            data: &line.data,
                            formula_errors: line.formula_errors.as_ref(),
                            created_by: ctx.user_id,
                            created_at: now,
                        },
                    )
                    .await?;
                }
            }
        }
    }
    Ok(())
}

/// 綁定明細的附件（同 [`apply_file_changes`]，資料類別為明細子表、資料為該列明細）。
async fn apply_line_files(
    tx: &mut PgTransaction<'_>,
    ctx: &AuthContext,
    details: &[DetailWrite<'_>],
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<(), CoreError> {
    for detail in details {
        for line in detail.lines.iter().filter(|l| !l.files.is_empty()) {
            apply_file_changes(
                tx,
                ctx,
                &detail.detail.entity.key,
                line.id,
                &line.files,
                client_info,
                now,
            )
            .await?;
        }
    }
    Ok(())
}

/// 寫入明細失敗時的錯誤。違反「不可重複」的唯一索引時 rollback 後找出與其他主表的明細重複的欄位
/// （同一次存檔內的重複已在 [`duplicate_line_values`] 檢查）。
async fn line_write_error(
    pool: &PgPool,
    tx: PgTransaction<'_>,
    err: DbError,
    parent_id: Id,
    details: &[DetailWrite<'_>],
) -> CoreError {
    if err.unique_violation().is_none() {
        return err.into();
    }
    // transaction 已中止，不能再查詢
    if let Err(e) = tx.rollback().await {
        return DbError::from(e).into();
    }
    let mut errors = Vec::new();
    for detail in details {
        let key = &detail.detail.entity.key;
        for line in detail
            .lines
            .iter()
            .filter(|l| l.action() != LineAction::Unchanged)
        {
            for field in detail.detail.fields.iter().filter(|f| f.is_unique) {
                let Some(value) = line.data.get(&field.field_key) else {
                    continue;
                };
                let taken = match &detail.detail.store {
                    LineStore::CustomRecords => {
                        db::line_value_taken(
                            pool,
                            detail.detail.entity.id,
                            parent_id,
                            &field.field_key,
                            value,
                        )
                        .await
                    }
                    LineStore::Table(table) => {
                        system_db::line_value_taken(pool, table, parent_id, &field.field_key, value)
                            .await
                    }
                };
                match taken {
                    Ok(true) => errors.push(LineError::field(
                        key,
                        line.index,
                        &field.field_key,
                        VALUE_TAKEN,
                    )),
                    Ok(false) => {}
                    Err(e) => return e.into(),
                }
            }
        }
    }
    if errors.is_empty() {
        CoreError::Validation(DUPLICATE_RECORD.into())
    } else {
        CoreError::InvalidLines {
            fields: Vec::new(),
            lines: errors,
        }
    }
}

/// 主表資料 `parent_id` 的明細，鍵為**有效的**明細子表代碼（沒有明細時為空陣列），依 `sort_order`。
async fn fetch_lines(
    conn: &mut PgConnection,
    details: &[Detail],
    parent_id: Id,
) -> Result<BTreeMap<String, Vec<Record>>, CoreError> {
    let rows = detail_lines(conn, details, parent_id).await?;
    let mut out = BTreeMap::new();
    for (detail, rows) in details.iter().zip(rows) {
        if !detail.entity.is_active {
            continue;
        }
        let titles = Titles::load(conn, &detail.entity, &detail.fields).await?;
        let rows = rows.into_iter().map(|r| r.into_record_row()).collect();
        let mut lines = titles.records(conn, rows).await?;
        attach_relations(conn, &detail.fields, &mut lines).await?;
        attach_files(conn, &detail.fields, &mut lines).await?;
        out.insert(detail.entity.key.clone(), lines);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hammer_shared::pagination::{DEFAULT_PAGE_SIZE, field as page_field};
    use hammer_shared::record::RecordSort;
    use hammer_shared::schema::SelectOption;

    fn field(key: &str, data_type: DataType) -> FieldDefinition {
        FieldDefinition {
            id: new_id(),
            entity_id: Id::nil(),
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
            options: data_type.uses_options().then(|| {
                vec![SelectOption {
                    value: "a".into(),
                    label: "A".into(),
                    color: None,
                }]
            }),
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

    fn fields() -> Vec<FieldDefinition> {
        let mut old = field("old", DataType::Text);
        old.is_active = false;
        let mut sys = field("code", DataType::Text);
        sys.storage = FieldStorage::Column;
        vec![
            field("name", DataType::Text),
            field("note", DataType::LongText),
            field("qty", DataType::Integer),
            field("rate", DataType::Decimal),
            field("price", DataType::Money),
            field("due", DataType::Date),
            field("at", DataType::DateTime),
            field("done", DataType::Boolean),
            field("status", DataType::Select),
            field("tags", DataType::MultiSelect),
            field("rel", DataType::Relation),
            old,
            sys,
        ]
    }

    fn query(pairs: &[(&str, &str)]) -> RecordListQuery {
        RecordListQuery::from_pairs(pairs).unwrap()
    }

    fn plan(pairs: &[(&str, &str)]) -> Result<(RecordListFilter, PageRequest), CoreError> {
        plan_list(Id::nil(), &fields(), &query(pairs))
    }

    fn conditions(pairs: &[(&str, &str)]) -> Vec<RecordCondition> {
        plan(pairs).unwrap().0.conditions
    }

    fn invalid_param(pairs: &[(&str, &str)]) -> (String, String) {
        match plan(pairs) {
            Err(CoreError::InvalidParam { param, message }) => (param, message),
            other => panic!("{pairs:?} 預期 InvalidParam，實際為 {other:?}"),
        }
    }

    fn compare(key: &str, cast: JsonCast, op: CompareOp, value: &str) -> RecordCondition {
        RecordCondition::Compare {
            field_key: key.into(),
            cast,
            op,
            value: value.into(),
        }
    }

    #[test]
    fn empty_query_uses_defaults() {
        let (filter, page) = plan(&[]).unwrap();
        assert_eq!(filter.conditions, []);
        assert_eq!(filter.order, None);
        assert_eq!(page.page_size, DEFAULT_PAGE_SIZE);
    }

    #[test]
    fn sort_uses_typed_cast() {
        for (key, cast) in [
            ("name", JsonCast::Text),
            ("qty", JsonCast::Numeric),
            ("rate", JsonCast::Numeric),
            ("price", JsonCast::Numeric),
            ("due", JsonCast::Date),
            ("at", JsonCast::Timestamptz),
            ("done", JsonCast::Boolean),
            ("status", JsonCast::Text),
            ("tags", JsonCast::Jsonb),
        ] {
            let (filter, _) = plan(&[("sort", &format!("-{key}"))]).unwrap();
            assert_eq!(
                filter.order,
                Some(RecordOrder {
                    target: OrderTarget::Field {
                        field_key: key.into(),
                        cast,
                    },
                    descending: true,
                }),
                "{key}"
            );
        }
    }

    #[test]
    fn sort_and_filter_by_record_no() {
        let (filter, _) = plan(&[
            ("sort", "-record_no"),
            ("filter.record_no.eq", "SO202611-0001"),
            ("filter.record_no.contains", "2611"),
        ])
        .unwrap();
        assert_eq!(
            filter.order,
            Some(RecordOrder {
                target: OrderTarget::RecordNo,
                descending: true,
            })
        );
        assert_eq!(
            filter.conditions,
            vec![
                RecordCondition::RecordNoEq("SO202611-0001".into()),
                RecordCondition::RecordNoContains("2611".into()),
            ]
        );
    }

    #[test]
    fn record_no_filter_supports_only_eq_and_contains() {
        for param in [
            "filter.record_no.gt",
            "filter.record_no.empty",
            "filter.record_no.any",
        ] {
            let (p, message) = invalid_param(&[(param, "1")]);
            assert_eq!(p, param);
            assert!(message.contains("單號"), "{message}");
        }
    }

    #[test]
    fn sort_rejects_unknown_inactive_column_and_unsupported_fields() {
        for key in ["missing", "old", "rel"] {
            let (param, _) = invalid_param(&[("sort", key)]);
            assert_eq!(param, "sort", "{key}");
        }
    }

    #[test]
    fn eq_for_each_comparable_type() {
        assert_eq!(
            conditions(&[
                ("filter.name.eq", "王小明"),
                ("filter.qty.eq", " 10 "),
                ("filter.rate.eq", "1.50"),
                ("filter.due.eq", "2026-11-13"),
                ("filter.at.eq", "2026-11-13T08:00:00+08:00"),
                ("filter.done.eq", "true"),
                ("filter.status.eq", "a"),
            ]),
            vec![
                compare("name", JsonCast::Text, CompareOp::Eq, "王小明"),
                compare("qty", JsonCast::Numeric, CompareOp::Eq, "10"),
                compare("rate", JsonCast::Numeric, CompareOp::Eq, "1.50"),
                compare("due", JsonCast::Date, CompareOp::Eq, "2026-11-13"),
                compare(
                    "at",
                    JsonCast::Timestamptz,
                    CompareOp::Eq,
                    "2026-11-13T08:00:00+08:00"
                ),
                compare("done", JsonCast::Boolean, CompareOp::Eq, "true"),
                compare("status", JsonCast::Text, CompareOp::Eq, "a"),
            ]
        );
    }

    #[test]
    fn relation_supports_only_eq_and_empty() {
        let id = "0190c3a2-0000-7000-8000-000000000000";
        assert_eq!(
            conditions(&[
                ("filter.rel.eq", "0190C3A2-0000-7000-8000-000000000000"),
                ("filter.rel.empty", "false"),
            ]),
            vec![
                // 比較值統一為存值格式（小寫連字號）
                compare("rel", JsonCast::Text, CompareOp::Eq, id),
                RecordCondition::HasValue {
                    field_key: "rel".into(),
                    has_value: true,
                },
            ]
        );
        for op in ["contains", "gt", "gte", "lt", "lte", "any"] {
            let name = format!("filter.rel.{op}");
            let (param, _) = invalid_param(&[(name.as_str(), id)]);
            assert_eq!(param, name);
        }
        let (param, message) = invalid_param(&[("filter.rel.eq", "abc")]);
        assert_eq!(param, "filter.rel.eq");
        assert_eq!(message, "資料 id 格式不正確");
    }

    #[test]
    fn range_operators() {
        assert_eq!(
            conditions(&[
                ("filter.qty.gt", "1"),
                ("filter.price.gte", "-0.5"),
                ("filter.due.lt", "2026-12-01"),
                ("filter.at.lte", "2026-11-13T00:00:00Z"),
            ]),
            vec![
                compare("qty", JsonCast::Numeric, CompareOp::Gt, "1"),
                compare("price", JsonCast::Numeric, CompareOp::Gte, "-0.5"),
                compare("due", JsonCast::Date, CompareOp::Lt, "2026-12-01"),
                compare(
                    "at",
                    JsonCast::Timestamptz,
                    CompareOp::Lte,
                    "2026-11-13T00:00:00Z"
                ),
            ]
        );
    }

    #[test]
    fn contains_empty_and_any() {
        assert_eq!(
            conditions(&[
                ("filter.note.contains", "50%"),
                ("filter.name.empty", "true"),
                ("filter.qty.empty", "false"),
                ("filter.tags.any", "a"),
                ("filter.status.eq", "a"),
                ("filter.tags.any", "b"),
            ]),
            vec![
                RecordCondition::Contains {
                    field_key: "note".into(),
                    text: "50%".into(),
                },
                RecordCondition::HasValue {
                    field_key: "name".into(),
                    has_value: false,
                },
                RecordCondition::HasValue {
                    field_key: "qty".into(),
                    has_value: true,
                },
                // 同一個欄位的 any 合併成一個條件
                RecordCondition::ContainsAny {
                    field_key: "tags".into(),
                    values: vec!["a".into(), "b".into()],
                },
                compare("status", JsonCast::Text, CompareOp::Eq, "a"),
            ]
        );
    }

    #[test]
    fn operator_must_match_type() {
        for (param, value) in [
            ("filter.qty.contains", "1"),
            ("filter.name.gt", "a"),
            ("filter.done.gte", "true"),
            ("filter.status.contains", "a"),
            ("filter.tags.eq", "a"),
            ("filter.name.any", "a"),
            ("filter.status.any", "a"),
        ] {
            assert_eq!(invalid_param(&[(param, value)]).0, param);
        }
    }

    #[test]
    fn values_must_match_type() {
        for (param, value) in [
            ("filter.qty.eq", "1.5"),
            ("filter.qty.gte", "abc"),
            ("filter.rate.eq", "1e3"),
            ("filter.due.eq", "2026/11/13"),
            ("filter.at.gte", "2026-11-13"),
            ("filter.done.eq", "yes"),
            ("filter.name.empty", "1"),
        ] {
            assert_eq!(invalid_param(&[(param, value)]).0, param, "{value}");
        }
    }

    #[test]
    fn filter_rejects_unknown_and_inactive_fields() {
        for param in ["filter.missing.eq", "filter.old.eq"] {
            let (p, message) = invalid_param(&[(param, "x")]);
            assert_eq!(p, param);
            assert!(message.contains("沒有可查詢的欄位"), "{message}");
        }
    }

    #[test]
    fn system_fields_can_be_filtered_and_sorted() {
        let (filter, _) = plan(&[("sort", "code"), ("filter.code.contains", "A")]).unwrap();
        assert_eq!(
            filter.order,
            Some(RecordOrder {
                target: OrderTarget::Field {
                    field_key: "code".into(),
                    cast: JsonCast::Text,
                },
                descending: false,
            })
        );
        assert_eq!(
            filter.conditions,
            vec![RecordCondition::Contains {
                field_key: "code".into(),
                text: "A".into(),
            }]
        );
    }

    #[test]
    fn system_field_types_map_to_columns() {
        for (data_type, expected) in [
            (DataType::Text, Some(ColumnType::Text)),
            (DataType::LongText, Some(ColumnType::Text)),
            (DataType::Select, Some(ColumnType::Text)),
            (DataType::Integer, Some(ColumnType::Integer)),
            (DataType::Decimal, Some(ColumnType::Numeric)),
            (DataType::Money, Some(ColumnType::Numeric)),
            (DataType::Date, Some(ColumnType::Date)),
            (DataType::DateTime, Some(ColumnType::Timestamptz)),
            (DataType::Boolean, Some(ColumnType::Boolean)),
            (DataType::Relation, Some(ColumnType::Uuid)),
            (DataType::MultiSelect, None),
            (DataType::File, None),
            (DataType::Formula, None),
        ] {
            assert_eq!(column_type(data_type), expected, "{data_type:?}");
        }
    }

    #[test]
    fn system_table_requires_matching_column_names() {
        let mut code = field("code", DataType::Text);
        code.storage = FieldStorage::Column;
        let mut tags = field("tags", DataType::MultiSelect);
        tags.storage = FieldStorage::Column;
        let fields = vec![code, tags, field("region", DataType::Text)];

        let table = system_table(
            Id::nil(),
            "units",
            &fields,
            &[("code".into(), Some("code".into()))],
        )
        .unwrap();
        assert_eq!(table.name(), "units");
        assert!(table.column("code").is_some());
        assert!(table.column("region").is_none());

        for columns in [
            vec![("code".to_owned(), Some("other".to_owned()))],
            vec![("code".to_owned(), None)],
            vec![("tags".to_owned(), Some("tags".to_owned()))],
            vec![("missing".to_owned(), Some("missing".to_owned()))],
        ] {
            assert!(
                matches!(
                    system_table(Id::nil(), "units", &fields, &columns),
                    Err(CoreError::Internal(_))
                ),
                "{columns:?}"
            );
        }
        assert!(matches!(
            system_table(Id::nil(), "units; --", &fields, &[]),
            Err(CoreError::Internal(_))
        ));
    }

    #[test]
    fn custom_data_excludes_system_columns() {
        let table = SystemTable::new(
            Id::nil(),
            "units",
            vec![SystemColumn {
                name: "code".into(),
                column_type: ColumnType::Text,
            }],
        )
        .unwrap();
        assert_eq!(
            custom_data_of(
                &table,
                &serde_json::json!({ "code": "pcs", "region": "北" })
            ),
            serde_json::json!({ "region": "北" })
        );
        assert_eq!(custom_data_of(&table, &Value::Null), serde_json::json!({}));
    }

    fn formula_fields() -> Vec<FieldDefinition> {
        let formula = |key: &str, result: Option<DataType>| FieldDefinition {
            formula: Some("1".into()),
            formula_result_type: result,
            ..field(key, DataType::Formula)
        };
        vec![
            formula("total", Some(DataType::Money)),
            formula("label", Some(DataType::Text)),
            formula("due", Some(DataType::Date)),
            formula("broken", None),
        ]
    }

    #[test]
    fn formula_fields_use_result_type() {
        let fields = formula_fields();
        let plan = |pairs: &[(&str, &str)]| plan_list(Id::nil(), &fields, &query(pairs));

        let (filter, _) = plan(&[
            ("sort", "-total"),
            ("filter.total.gte", "10"),
            ("filter.label.contains", "A"),
            ("filter.due.lt", "2026-12-01"),
            ("filter.total.empty", "true"),
        ])
        .unwrap();
        assert_eq!(
            filter.order,
            Some(RecordOrder {
                target: OrderTarget::Field {
                    field_key: "total".into(),
                    cast: JsonCast::Numeric,
                },
                descending: true,
            })
        );
        assert_eq!(
            filter.conditions,
            vec![
                compare("total", JsonCast::Numeric, CompareOp::Gte, "10"),
                RecordCondition::Contains {
                    field_key: "label".into(),
                    text: "A".into(),
                },
                compare("due", JsonCast::Date, CompareOp::Lt, "2026-12-01"),
                RecordCondition::HasValue {
                    field_key: "total".into(),
                    has_value: false,
                },
            ]
        );

        // 運算子與值依結果型態檢查
        for (param, value) in [
            ("filter.label.gt", "a"),
            ("filter.total.contains", "1"),
            ("filter.total.eq", "abc"),
            ("filter.due.eq", "2026/12/01"),
        ] {
            match plan(&[(param, value)]) {
                Err(CoreError::InvalidParam { param: p, .. }) => assert_eq!(p, param),
                other => panic!("{param} 預期 InvalidParam，實際為 {other:?}"),
            }
        }
        // 沒有結果型態的公式欄位不能查詢
        for pairs in [[("sort", "broken")], [("filter.broken.empty", "true")]] {
            assert!(
                matches!(plan(&pairs), Err(CoreError::InvalidParam { .. })),
                "{pairs:?}"
            );
        }
    }

    #[test]
    fn invalid_page_is_invalid_param() {
        assert_eq!(invalid_param(&[("page", "0")]).0, page_field::PAGE);
        assert_eq!(
            invalid_param(&[("page_size", "0")]).0,
            page_field::PAGE_SIZE
        );
    }

    #[test]
    fn plan_passes_entity_id_and_sort_direction() {
        let entity_id = new_id();
        let q = RecordListQuery {
            sort: Some(RecordSort {
                field_key: "qty".into(),
                descending: false,
            }),
            ..Default::default()
        };
        let (filter, _) = plan_list(entity_id, &fields(), &q).unwrap();
        assert_eq!(filter.entity_id, entity_id);
        assert!(!filter.order.unwrap().descending);
    }

    #[test]
    fn record_row_conversion() {
        let row = RecordRow {
            id: new_id(),
            entity_id: new_id(),
            record_no: Some("RO-001".into()),
            data: serde_json::json!({ "qty": 1 }),
            formula_errors: Some(serde_json::json!({ "total": "除數不可為零" })),
            created_at: OffsetDateTime::UNIX_EPOCH,
            created_by: None,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            updated_by: None,
            version: 3,
        };
        let titles = TitleRenderer::new(None, &[]);
        let r = record_from_row(row.clone(), &titles, &RelatedRecords::new()).unwrap();
        assert_eq!(r.data["qty"], 1);
        assert_eq!(r.record_no.as_deref(), Some("RO-001"));
        assert_eq!(r.title, "RO-001");
        assert_eq!(r.version, 3);
        assert_eq!(r.formula_errors["total"], "除數不可為零");
        let bad = RecordRow {
            data: serde_json::json!([1]),
            ..row
        };
        assert!(matches!(
            record_from_row(bad, &titles, &RelatedRecords::new()),
            Err(CoreError::Internal(_))
        ));
    }
}

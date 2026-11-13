//! 自訂資料類別的資料（docs/architecture.md §6.1、§7.1、§7.5、§8.5 `custom_records`、§9.4、§10）。
//!
//! - 讀取（需要 `record.read`）：[`list_records`]、[`get_record`]
//! - 寫入（需要 `record.write`）：[`create_record`]、[`update_record`]、[`delete_record`]
//!
//! 只處理有效（啟用、未刪除）且 `storage = custom_records` 的資料類別，其他一律回傳
//! [`CoreError::NotFound`]。`data` 依欄位定義驗證（[`crate::record_data`]）。
//!
//! 寫入都在單一 transaction 內完成，並寫入一筆 `audit_logs`：`entity_key` 為資料類別代碼，
//! `before_data` / `after_data` 只包含有變動的欄位（[`AuditDiff`]）。修改沒有任何變動時不寫入。
//!
//! 違反「不可重複」欄位的唯一索引（§7.5）時，找出與其他資料重複的欄位，回傳
//! [`CoreError::InvalidFields`]（`422 VALIDATION`）。
//!
//! 列表的 query string 格式見 [`hammer_shared::record`]；欄位與運算子的檢查在 [`plan_list`]。

use hammer_db::DbError;
use hammer_db::ensure_version_matched;
use hammer_db::record::{
    self as db, CompareOp, JsonCast, NewRecord, RecordCondition, RecordListFilter, RecordOrder,
    RecordRow,
};
use hammer_db::schema as schema_db;
use hammer_db::sqlx::{PgConnection, PgPool, PgTransaction};
use hammer_shared::pagination::{Page, PageRequest};
use hammer_shared::record::{
    CreateRecordRequest, FilterOp, QueryParamError, Record, RecordFilter, RecordListQuery,
    UpdateRecordRequest, query_param,
};
use hammer_shared::schema::{
    DataType, EntityDefinition, EntityStorage, FieldDefinition, FieldStorage, parse_value, to_json,
};
use hammer_shared::{Id, audit as action, new_id, permission};
use serde_json::{Map, Value};
use time::OffsetDateTime;

use crate::CoreError;
use crate::audit::{self, AuditDiff, AuditEntry};
use crate::auth::AuthContext;
use crate::record_data::{FieldError, WriteMode, normalize_record_data};
use crate::schema::{ENTITY_NOT_FOUND, ensure_version, entity_from_row, field_from_row};

/// 資料不存在或已刪除時的錯誤訊息。
pub const RECORD_NOT_FOUND: &str = "找不到此資料";
/// 「不可重複」欄位的值已被其他資料使用時的錯誤訊息。
pub const VALUE_TAKEN: &str = "此值已被其他資料使用";
/// 違反唯一索引，但找不到重複的欄位時（例如另一筆資料剛好被刪除）的錯誤訊息。
pub const DUPLICATE_RECORD: &str = "資料與其他資料重複，請重新確認後再送出";

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
    let (entity, fields) = load_entity(&mut conn, entity_key).await?;

    let query = RecordListQuery::from_pairs(pairs).map_err(|err| match err {
        QueryParamError::NotInteger { .. } => CoreError::BadRequest(err.message()),
        QueryParamError::Invalid { param, message } => CoreError::InvalidParam { param, message },
    })?;
    let (filter, page) = plan_list(entity.id, &fields, &query)?;

    let total = db::count_records(&mut *conn, &filter).await?;
    let rows = db::list_records(&mut *conn, &filter, page.limit(), page.offset()).await?;
    let items = rows
        .into_iter()
        .map(record_from_row)
        .collect::<Result<Vec<_>, _>>()?;
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
    let (entity, _) = load_entity(&mut conn, entity_key).await?;
    let row = db::find_record(&mut *conn, entity.id, id)
        .await?
        .ok_or_else(record_not_found)?;
    record_from_row(row)
}

// ---------------------------------------------------------------------------
// 寫入
// ---------------------------------------------------------------------------

/// 新增資料。需要 `record.write`。
///
/// - `data` 不符合欄位定義、違反「不可重複」→ [`CoreError::InvalidFields`]
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
    let (entity, fields) = load_entity_for_write(&mut tx, entity_key).await?;
    let data = Value::Object(
        normalize_record_data(&fields, &req.data, WriteMode::Create)
            .map_err(CoreError::InvalidFields)?,
    );

    let id = new_id();
    let inserted = db::insert_record(
        &mut *tx,
        &NewRecord {
            id,
            entity_id: entity.id,
            data: &data,
            created_by: ctx.user_id,
            created_at: now,
        },
    )
    .await;
    if let Err(err) = inserted {
        return Err(write_error(pool, tx, err, entity.id, id, &fields, &data).await);
    }

    record_audit(
        &mut tx,
        ctx,
        action::CREATE,
        &entity.key,
        id,
        AuditDiff::between(None, Some(&data))?,
        client_info,
        now,
    )
    .await?;
    let record = fetch_record(&mut tx, entity.id, id).await?;
    commit(tx).await?;

    tracing::info!(actor = %ctx.user_id, entity = %entity.key, record_id = %id, "已新增資料");
    Ok(record)
}

/// 修改資料：只合併有帶到的欄位，`null` 代表清空。需要 `record.write`。
///
/// - 資料不存在或已刪除 → [`CoreError::NotFound`]
/// - `version` 不符 → `Conflict`（§9.4）
/// - `data` 不符合欄位定義、違反「不可重複」→ [`CoreError::InvalidFields`]
/// - 沒有任何變動時不寫入，回傳目前的資料（`version` 不變）
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
    let (entity, fields) = load_entity_for_write(&mut tx, entity_key).await?;
    let current = lock_record(&mut tx, entity.id, id).await?;
    ensure_version(current.version, req.version)?;

    let existing = data_object(&current.data)?;
    let data = Value::Object(
        normalize_record_data(&fields, &req.data, WriteMode::Update { existing })
            .map_err(CoreError::InvalidFields)?,
    );
    if data == current.data {
        commit(tx).await?;
        return record_from_row(current);
    }

    let updated = db::update_record_data(&mut *tx, id, req.version, &data, ctx.user_id).await;
    match updated {
        Ok(result) => {
            ensure_version_matched(result)?;
        }
        Err(err) => return Err(write_error(pool, tx, err, entity.id, id, &fields, &data).await),
    }

    record_audit(
        &mut tx,
        ctx,
        action::UPDATE,
        &entity.key,
        id,
        AuditDiff::between(Some(&current.data), Some(&data))?,
        client_info,
        now,
    )
    .await?;
    let record = fetch_record(&mut tx, entity.id, id).await?;
    commit(tx).await?;

    tracing::info!(actor = %ctx.user_id, entity = %entity.key, record_id = %id, "已修改資料");
    Ok(record)
}

/// 軟刪除資料。需要 `record.write`。
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
    let (entity, _) = load_entity_for_write(&mut tx, entity_key).await?;
    let current = lock_record(&mut tx, entity.id, id).await?;
    ensure_version(current.version, version)?;

    ensure_version_matched(db::soft_delete_record(&mut *tx, id, version, now, ctx.user_id).await?)?;
    record_audit(
        &mut tx,
        ctx,
        action::DELETE,
        &entity.key,
        id,
        AuditDiff::between(Some(&current.data), None)?,
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
/// - 只能使用有效（`is_active`）、`storage = json` 且目前支援的欄位
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
            let field = queryable_field(fields, &sort.field_key, query_param::SORT)?;
            let cast = sort_cast(field.data_type).ok_or_else(|| CoreError::InvalidParam {
                param: query_param::SORT.to_owned(),
                message: "此欄位型態不支援排序".to_owned(),
            })?;
            Ok::<_, CoreError>(RecordOrder {
                field_key: field.field_key.clone(),
                cast,
                descending: sort.descending,
            })
        })
        .transpose()?;

    let mut conditions: Vec<RecordCondition> = Vec::new();
    for filter in &query.filters {
        let param = filter.param_name();
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

/// 可用於篩選與排序的欄位：有效、`storage = json`、目前支援的型態。
fn queryable_field<'a>(
    fields: &'a [FieldDefinition],
    field_key: &str,
    param: &str,
) -> Result<&'a FieldDefinition, CoreError> {
    fields
        .iter()
        .find(|f| {
            f.field_key == field_key
                && f.is_active
                && f.storage == FieldStorage::Json
                && f.data_type.is_value_supported()
        })
        .ok_or_else(|| CoreError::InvalidParam {
            param: param.to_owned(),
            message: format!("沒有可查詢的欄位「{field_key}」"),
        })
}

/// 比較與排序時的轉型方式；`multi_select` 只能排序不能比較。
fn compare_cast(data_type: DataType) -> Option<JsonCast> {
    match data_type {
        DataType::Text | DataType::LongText | DataType::Select => Some(JsonCast::Text),
        DataType::Integer | DataType::Decimal | DataType::Money => Some(JsonCast::Numeric),
        DataType::Date => Some(JsonCast::Date),
        DataType::DateTime => Some(JsonCast::Timestamptz),
        DataType::Boolean => Some(JsonCast::Boolean),
        DataType::MultiSelect | DataType::Relation | DataType::File | DataType::Formula => None,
    }
}

fn sort_cast(data_type: DataType) -> Option<JsonCast> {
    match data_type {
        DataType::MultiSelect => Some(JsonCast::Jsonb),
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
    if !op_supported(field.data_type, filter.op) {
        return Err(format!("此欄位型態不支援 {} 篩選", filter.op.as_str()));
    }
    let field_key = field.field_key.clone();
    let compare = |op: CompareOp| -> Result<RecordCondition, String> {
        Ok(RecordCondition::Compare {
            field_key: field.field_key.clone(),
            // op_supported 已確認可比較
            cast: compare_cast(field.data_type).unwrap_or(JsonCast::Text),
            op,
            value: compare_value(field.data_type, &filter.value)?,
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

/// 載入有效的自訂資料類別與其所有欄位定義（含已停用）。
async fn load_entity(
    conn: &mut PgConnection,
    key: &str,
) -> Result<(EntityDefinition, Vec<FieldDefinition>), CoreError> {
    let row = schema_db::find_entity_by_key(&mut *conn, key).await?;
    load_fields_of(conn, row).await
}

/// 同 [`load_entity`]，寫入資料時使用：以 `FOR SHARE` 鎖定資料類別，等待進行中的欄位定義變更
/// （例如變更欄位型態，[`crate::schema::change_field_type`]）完成後才讀取欄位定義，
/// 避免依舊的定義寫入資料。
async fn load_entity_for_write(
    tx: &mut PgTransaction<'_>,
    key: &str,
) -> Result<(EntityDefinition, Vec<FieldDefinition>), CoreError> {
    let row = schema_db::find_entity_by_key_for_share(&mut **tx, key).await?;
    load_fields_of(tx, row).await
}

async fn load_fields_of(
    conn: &mut PgConnection,
    row: Option<schema_db::EntityRow>,
) -> Result<(EntityDefinition, Vec<FieldDefinition>), CoreError> {
    let entity = row
        .map(entity_from_row)
        .transpose()?
        .filter(|e| e.is_active && e.storage == EntityStorage::CustomRecords)
        .ok_or_else(|| CoreError::NotFound(ENTITY_NOT_FOUND.into()))?;
    let fields = schema_db::list_fields(&mut *conn, entity.id)
        .await?
        .into_iter()
        .map(field_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    Ok((entity, fields))
}

/// 資料列轉成 API 回應。`data` 不是 JSON 物件時回傳 [`CoreError::Internal`]。
pub fn record_from_row(row: RecordRow) -> Result<Record, CoreError> {
    let Value::Object(data) = row.data else {
        return Err(CoreError::Internal(
            "custom_records.data 不是 JSON 物件".into(),
        ));
    };
    Ok(Record {
        id: row.id,
        data,
        created_at: row.created_at,
        created_by: row.created_by,
        updated_at: row.updated_at,
        updated_by: row.updated_by,
        version: row.version,
    })
}

fn data_object(data: &Value) -> Result<&Map<String, Value>, CoreError> {
    data.as_object()
        .ok_or_else(|| CoreError::Internal("custom_records.data 不是 JSON 物件".into()))
}

/// 寫入 `custom_records` 失敗時的錯誤。違反唯一索引時 rollback 後找出重複的欄位。
async fn write_error(
    pool: &PgPool,
    tx: PgTransaction<'_>,
    err: DbError,
    entity_id: Id,
    record_id: Id,
    fields: &[FieldDefinition],
    data: &Value,
) -> CoreError {
    if err.unique_violation().is_none() {
        return err.into();
    }
    // transaction 已中止，不能再查詢
    if let Err(e) = tx.rollback().await {
        return DbError::from(e).into();
    }
    match duplicate_fields(pool, entity_id, record_id, fields, data).await {
        Ok(errors) if errors.is_empty() => CoreError::Validation(DUPLICATE_RECORD.into()),
        Ok(errors) => CoreError::InvalidFields(errors),
        Err(e) => e,
    }
}

/// 「不可重複」欄位中，值與其他資料相同的欄位。
async fn duplicate_fields(
    pool: &PgPool,
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
        if db::value_taken(pool, entity_id, record_id, &field.field_key, value).await? {
            errors.push(FieldError {
                field_key: field.field_key.clone(),
                message: VALUE_TAKEN.to_owned(),
            });
        }
    }
    Ok(errors)
}

#[allow(clippy::too_many_arguments)]
async fn record_audit(
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

async fn lock_record(
    tx: &mut PgTransaction<'_>,
    entity_id: Id,
    id: Id,
) -> Result<RecordRow, CoreError> {
    db::find_record_for_update(&mut **tx, entity_id, id)
        .await?
        .ok_or_else(record_not_found)
}

async fn fetch_record(
    tx: &mut PgTransaction<'_>,
    entity_id: Id,
    id: Id,
) -> Result<Record, CoreError> {
    let row = db::find_record(&mut **tx, entity_id, id)
        .await?
        .ok_or_else(|| CoreError::Internal("寫入後找不到資料".into()))?;
    record_from_row(row)
}

fn record_not_found() -> CoreError {
    CoreError::NotFound(RECORD_NOT_FOUND.into())
}

async fn commit(tx: PgTransaction<'_>) -> Result<(), CoreError> {
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
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
                    field_key: key.into(),
                    cast,
                    descending: true,
                }),
                "{key}"
            );
        }
    }

    #[test]
    fn sort_rejects_unknown_inactive_column_and_unsupported_fields() {
        for key in ["missing", "old", "code", "rel"] {
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
        for param in ["filter.missing.eq", "filter.old.eq", "filter.code.eq"] {
            let (p, message) = invalid_param(&[(param, "x")]);
            assert_eq!(p, param);
            assert!(message.contains("沒有可查詢的欄位"), "{message}");
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
            data: serde_json::json!({ "qty": 1 }),
            created_at: OffsetDateTime::UNIX_EPOCH,
            created_by: None,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            updated_by: None,
            version: 3,
        };
        let r = record_from_row(row.clone()).unwrap();
        assert_eq!(r.data["qty"], 1);
        assert_eq!(r.version, 3);
        let bad = RecordRow {
            data: serde_json::json!([1]),
            ..row
        };
        assert!(matches!(record_from_row(bad), Err(CoreError::Internal(_))));
    }
}

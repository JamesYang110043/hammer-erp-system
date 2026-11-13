//! 列表版面（docs/architecture.md §6.1、§8.5 `view_layouts`、§9.4、§10）。
//!
//! - 查詢（需要 `record.read`）：[`list_layouts`]、[`get_default_layout`]
//! - 寫入：[`create_layout`]、[`update_layout`]、[`delete_layout`]。個人版面只有本人可以修改
//!   （需要 `record.read`）；全域版面（`user_id` 為空）需要 `schema.manage`。
//!   別人的個人版面一律視為不存在（[`CoreError::NotFound`]）。
//!
//! 只處理有效（啟用、未刪除）的資料類別。端點、權限與錯誤的完整說明見 [`hammer_shared::layout`]。
//!
//! 寫入都在單一 transaction 內完成，並寫入 `audit_logs`（`entity_key` 為 [`LAYOUT_AUDIT_KEY`]，
//! 只記錄有變動的屬性）。設為預設時，同一範圍原本的預設版面在同一個 transaction 內取消，
//! 也各寫入一筆修改紀錄。修改沒有任何變動時不寫入。
//!
//! `config` 的檢查與讀取時的略過規則在 [`validate_config`]、[`sanitize_config`]、
//! [`generated_config`]（純邏輯，有單元測試）。排序與篩選沿用通用資料 API 的規則
//! （[`crate::records::plan_list`]）。

use std::collections::HashSet;

use hammer_db::DbError;
use hammer_db::ensure_version_matched;
use hammer_db::schema as schema_db;
use hammer_db::sqlx::{PgConnection, PgPool, PgTransaction};
use hammer_db::view_layout::{
    self as db, GLOBAL_DEFAULT_UNIQUE, LayoutRow, LayoutUpdate, NewLayout, USER_DEFAULT_UNIQUE,
};
use hammer_shared::layout::{
    CreateViewLayoutRequest, DefaultViewLayout, LayoutColumn, LayoutConfig, LayoutSource,
    MAX_LAYOUT_NAME_CHARS, UpdateViewLayoutRequest, ViewLayout,
};
use hammer_shared::record::RecordListQuery;
use hammer_shared::schema::{EntityDefinition, FieldDefinition};
use hammer_shared::{Id, audit as action, new_id, permission};
use serde_json::{Value, json};
use time::OffsetDateTime;

use crate::CoreError;
use crate::audit::{self, AuditDiff, AuditEntry};
use crate::auth::AuthContext;
use crate::records::plan_list;
use crate::schema::{ENTITY_NOT_FOUND, ensure_version, entity_from_row, field_from_row};

/// 列表版面在 `audit_logs.entity_key` 的值。
pub const LAYOUT_AUDIT_KEY: &str = "view_layout";
/// 版面不存在、已刪除或是別人的個人版面時的錯誤訊息。
pub const LAYOUT_NOT_FOUND: &str = "找不到此版面";
/// 同時有其他請求設定了同一範圍的預設版面時的錯誤訊息。
pub const DEFAULT_TAKEN: &str = "預設版面已被其他人修改，請重新載入";

/// 驗證錯誤 `details.field` 的欄位路徑。
pub mod field {
    pub const NAME: &str = "name";
    pub const SORT: &str = "config.sort";

    /// `config.columns[i].field_key`
    pub fn column_key(index: usize) -> String {
        format!("config.columns[{index}].field_key")
    }

    /// `config.columns[i].width`
    pub fn column_width(index: usize) -> String {
        format!("config.columns[{index}].width")
    }

    /// `config.filters[i]`
    pub fn filter(index: usize) -> String {
        format!("config.filters[{index}]")
    }
}

// ---------------------------------------------------------------------------
// 查詢
// ---------------------------------------------------------------------------

/// 列出全域版面與目前使用者自己的版面（全域在前，再依名稱）。需要 `record.read`。
pub async fn list_layouts(
    pool: &PgPool,
    ctx: &AuthContext,
    entity_key: &str,
) -> Result<Vec<ViewLayout>, CoreError> {
    ctx.require(permission::RECORD_READ)?;
    let mut conn = pool.acquire().await.map_err(DbError::from)?;
    let (entity, fields) = load_entity(&mut conn, entity_key).await?;
    let rows = db::list_layouts(&mut *conn, entity.id, ctx.user_id).await?;
    Ok(rows
        .into_iter()
        .map(|row| layout_from_row(row, &fields))
        .collect())
}

/// 取得要套用的預設版面：使用者自己的預設 → 全域預設 → 依欄位定義自動產生。
/// 需要 `record.read`。
pub async fn get_default_layout(
    pool: &PgPool,
    ctx: &AuthContext,
    entity_key: &str,
) -> Result<DefaultViewLayout, CoreError> {
    ctx.require(permission::RECORD_READ)?;
    let mut conn = pool.acquire().await.map_err(DbError::from)?;
    let (entity, fields) = load_entity(&mut conn, entity_key).await?;

    let candidates = [
        (Some(ctx.user_id), LayoutSource::Personal),
        (None, LayoutSource::Global),
    ];
    for (user_id, source) in candidates {
        if let Some(row) = db::find_default(&mut *conn, entity.id, user_id).await? {
            let layout = layout_from_row(row, &fields);
            return Ok(DefaultViewLayout {
                source,
                config: layout.config.clone(),
                layout: Some(layout),
            });
        }
    }
    Ok(DefaultViewLayout {
        source: LayoutSource::Generated,
        layout: None,
        config: generated_config(&fields),
    })
}

// ---------------------------------------------------------------------------
// 寫入
// ---------------------------------------------------------------------------

/// 新增版面。個人版面需要 `record.read`；全域版面（`is_global`）需要 `schema.manage`。
///
/// - 名稱、`config` 不合法 → [`CoreError::InvalidField`] / [`CoreError::InvalidParam`]
/// - `is_default` 時取消同一範圍原本的預設版面
pub async fn create_layout(
    pool: &PgPool,
    ctx: &AuthContext,
    entity_key: &str,
    req: &CreateViewLayoutRequest,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<ViewLayout, CoreError> {
    ctx.require(permission::RECORD_READ)?;
    if req.is_global {
        ctx.require(permission::SCHEMA_MANAGE)?;
    }
    let user_id = (!req.is_global).then_some(ctx.user_id);

    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let (entity, fields) = load_entity_for_write(&mut tx, entity_key).await?;
    let name = validate_name(&req.name)?;
    let config = config_json(&validate_config(entity.id, &fields, &req.config)?)?;

    let id = new_id();
    if req.is_default {
        clear_other_defaults(&mut tx, ctx, entity.id, user_id, id, client_info, now).await?;
    }
    db::insert_layout(
        &mut *tx,
        &NewLayout {
            id,
            entity_id: entity.id,
            user_id,
            name: &name,
            config: &config,
            is_default: req.is_default,
            created_by: ctx.user_id,
            created_at: now,
        },
    )
    .await
    .map_err(write_error)?;

    let after = snapshot(&entity.key, user_id, &name, &config, req.is_default);
    record_audit(
        &mut tx,
        ctx,
        action::CREATE,
        id,
        AuditDiff::between(None, Some(&after))?,
        client_info,
        now,
    )
    .await?;
    let layout = fetch_layout(&mut tx, entity.id, id, &fields).await?;
    commit(tx).await?;

    tracing::info!(actor = %ctx.user_id, entity = %entity.key, layout_id = %id, "已新增列表版面");
    Ok(layout)
}

/// 修改版面：只修改有帶到的屬性。權限依版面是個人或全域（見模組說明）。
///
/// - 版面不存在、已刪除或是別人的個人版面 → [`CoreError::NotFound`]
/// - `version` 不符 → `Conflict`（§9.4）
/// - `is_default` 改為 `true` 時取消同一範圍原本的預設版面
/// - 沒有任何變動時不寫入，回傳目前的版面（`version` 不變）
pub async fn update_layout(
    pool: &PgPool,
    ctx: &AuthContext,
    entity_key: &str,
    id: Id,
    req: &UpdateViewLayoutRequest,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<ViewLayout, CoreError> {
    ctx.require(permission::RECORD_READ)?;

    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let (entity, fields) = load_entity_for_write(&mut tx, entity_key).await?;
    let current = lock_layout(&mut tx, ctx, entity.id, id).await?;
    ensure_version(current.version, req.version)?;

    let name = match &req.name {
        Some(name) => validate_name(name)?,
        None => current.name.clone(),
    };
    let config = match &req.config {
        Some(config) => config_json(&validate_config(entity.id, &fields, config)?)?,
        None => current.config.clone(),
    };
    let is_default = req.is_default.unwrap_or(current.is_default);

    let before = snapshot(
        &entity.key,
        current.user_id,
        &current.name,
        &current.config,
        current.is_default,
    );
    let after = snapshot(&entity.key, current.user_id, &name, &config, is_default);
    if before == after {
        commit(tx).await?;
        return Ok(layout_from_row(current, &fields));
    }

    if is_default && !current.is_default {
        clear_other_defaults(
            &mut tx,
            ctx,
            entity.id,
            current.user_id,
            id,
            client_info,
            now,
        )
        .await?;
    }
    let result = db::update_layout(
        &mut *tx,
        id,
        req.version,
        &LayoutUpdate {
            name: &name,
            config: &config,
            is_default,
        },
        ctx.user_id,
    )
    .await
    .map_err(write_error)?;
    ensure_version_matched(result)?;

    record_audit(
        &mut tx,
        ctx,
        action::UPDATE,
        id,
        AuditDiff::between(Some(&before), Some(&after))?,
        client_info,
        now,
    )
    .await?;
    let layout = fetch_layout(&mut tx, entity.id, id, &fields).await?;
    commit(tx).await?;

    tracing::info!(actor = %ctx.user_id, entity = %entity.key, layout_id = %id, "已修改列表版面");
    Ok(layout)
}

/// 軟刪除版面。權限依版面是個人或全域（見模組說明）。
///
/// - 版面不存在、已刪除或是別人的個人版面 → [`CoreError::NotFound`]
/// - `version` 不符 → `Conflict`（§9.4）
pub async fn delete_layout(
    pool: &PgPool,
    ctx: &AuthContext,
    entity_key: &str,
    id: Id,
    version: i32,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<(), CoreError> {
    ctx.require(permission::RECORD_READ)?;

    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let (entity, _) = load_entity_for_write(&mut tx, entity_key).await?;
    let current = lock_layout(&mut tx, ctx, entity.id, id).await?;
    ensure_version(current.version, version)?;

    ensure_version_matched(db::soft_delete_layout(&mut *tx, id, version, now, ctx.user_id).await?)?;
    let before = snapshot(
        &entity.key,
        current.user_id,
        &current.name,
        &current.config,
        current.is_default,
    );
    record_audit(
        &mut tx,
        ctx,
        action::DELETE,
        id,
        AuditDiff::between(Some(&before), None)?,
        client_info,
        now,
    )
    .await?;
    commit(tx).await?;

    tracing::info!(actor = %ctx.user_id, entity = %entity.key, layout_id = %id, "已刪除列表版面");
    Ok(())
}

// ---------------------------------------------------------------------------
// config（純邏輯）
// ---------------------------------------------------------------------------

/// 檢查版面名稱：去除前後空白後不可為空，最多 [`MAX_LAYOUT_NAME_CHARS`] 個字元。
pub fn validate_name(name: &str) -> Result<String, CoreError> {
    let name = name.trim();
    let invalid = |message: String| CoreError::InvalidField {
        field: field::NAME,
        message,
    };
    if name.is_empty() {
        return Err(invalid("請輸入版面名稱".to_owned()));
    }
    if name.chars().count() > MAX_LAYOUT_NAME_CHARS {
        return Err(invalid(format!(
            "版面名稱最多 {MAX_LAYOUT_NAME_CHARS} 個字元"
        )));
    }
    Ok(name.to_owned())
}

/// 寫入前檢查 `config`，錯誤為 [`CoreError::InvalidParam`]（`param` 見 [`field`]）。
///
/// - 欄位必須是有效（未停用）的欄位，且不可重複；欄寬必須大於 0
/// - 排序與每個篩選條件依通用資料 API 的規則檢查（[`plan_list`]）
pub fn validate_config(
    entity_id: Id,
    fields: &[FieldDefinition],
    config: &LayoutConfig,
) -> Result<LayoutConfig, CoreError> {
    let mut seen = HashSet::new();
    for (i, column) in config.columns.iter().enumerate() {
        let invalid = |param: String, message: String| CoreError::InvalidParam { param, message };
        if !is_active_field(fields, &column.field_key) {
            return Err(invalid(
                field::column_key(i),
                format!("沒有可顯示的欄位「{}」", column.field_key),
            ));
        }
        if !seen.insert(column.field_key.as_str()) {
            return Err(invalid(
                field::column_key(i),
                format!("欄位「{}」重複", column.field_key),
            ));
        }
        if column.width.is_some_and(|w| w <= 0) {
            return Err(invalid(field::column_width(i), "欄寬必須大於 0".to_owned()));
        }
    }

    if config.sort.is_some() {
        let query = RecordListQuery {
            sort: config.sort.clone(),
            ..Default::default()
        };
        plan_list(entity_id, fields, &query).map_err(|err| with_param(err, field::SORT.into()))?;
    }
    for (i, filter) in config.filters.iter().enumerate() {
        let query = RecordListQuery {
            filters: vec![filter.clone()],
            ..Default::default()
        };
        plan_list(entity_id, fields, &query).map_err(|err| with_param(err, field::filter(i)))?;
    }
    Ok(config.clone())
}

/// 讀取時略過已不適用的部分，不回傳錯誤：
///
/// - 顯示欄位：已停用或不存在的欄位，以及重複的欄位
/// - 排序、篩選條件：參照到已停用或不存在的欄位，或已不符合規則
///   （例如欄位型態變更後運算子不再適用）
pub fn sanitize_config(
    entity_id: Id,
    fields: &[FieldDefinition],
    config: LayoutConfig,
) -> LayoutConfig {
    let mut seen = HashSet::new();
    let columns = config
        .columns
        .into_iter()
        .filter(|c| is_active_field(fields, &c.field_key) && seen.insert(c.field_key.clone()))
        .collect();
    let sort = config.sort.filter(|sort| {
        let query = RecordListQuery {
            sort: Some(sort.clone()),
            ..Default::default()
        };
        plan_list(entity_id, fields, &query).is_ok()
    });
    let filters = config
        .filters
        .into_iter()
        .filter(|filter| {
            let query = RecordListQuery {
                filters: vec![filter.clone()],
                ..Default::default()
            };
            plan_list(entity_id, fields, &query).is_ok()
        })
        .collect();
    LayoutConfig {
        columns,
        sort,
        filters,
    }
}

/// 沒有預設版面時依欄位定義產生：有效且顯示的欄位，依 `sort_order`（同值依 id），
/// 欄寬為欄位定義的 `column_width`；不排序、不篩選。
pub fn generated_config(fields: &[FieldDefinition]) -> LayoutConfig {
    let mut visible: Vec<&FieldDefinition> = fields
        .iter()
        .filter(|f| f.is_active && f.is_visible)
        .collect();
    visible.sort_by_key(|f| (f.sort_order, f.id));
    LayoutConfig {
        columns: visible
            .into_iter()
            .map(|f| LayoutColumn {
                field_key: f.field_key.clone(),
                width: f.column_width,
            })
            .collect(),
        sort: None,
        filters: Vec::new(),
    }
}

fn is_active_field(fields: &[FieldDefinition], field_key: &str) -> bool {
    fields
        .iter()
        .any(|f| f.field_key == field_key && f.is_active)
}

/// 把 [`plan_list`] 的參數錯誤改成 `config` 內的路徑。
fn with_param(err: CoreError, param: String) -> CoreError {
    match err {
        CoreError::InvalidParam { message, .. } => CoreError::InvalidParam { param, message },
        other => other,
    }
}

// ---------------------------------------------------------------------------
// 共用
// ---------------------------------------------------------------------------

/// 資料列轉成 API 回應，並略過 `config` 中已不適用的部分（[`sanitize_config`]）。
/// `config` 無法解析時（不應發生）視為空的設定，只寫進 log。
fn layout_from_row(row: LayoutRow, fields: &[FieldDefinition]) -> ViewLayout {
    let config = serde_json::from_value::<LayoutConfig>(row.config).unwrap_or_else(|err| {
        tracing::warn!(layout_id = %row.id, error = %err, "view_layouts.config 格式錯誤，視為空的設定");
        LayoutConfig::default()
    });
    ViewLayout {
        id: row.id,
        user_id: row.user_id,
        name: row.name,
        config: sanitize_config(row.entity_id, fields, config),
        is_default: row.is_default,
        created_at: row.created_at,
        updated_at: row.updated_at,
        version: row.version,
    }
}

fn config_json(config: &LayoutConfig) -> Result<Value, CoreError> {
    serde_json::to_value(config)
        .map_err(|e| CoreError::Internal(format!("序列化版面設定失敗：{e}")))
}

/// 操作紀錄的快照。
fn snapshot(
    entity_key: &str,
    user_id: Option<Id>,
    name: &str,
    config: &Value,
    is_default: bool,
) -> Value {
    json!({
        "entity_key": entity_key,
        "user_id": user_id,
        "name": name,
        "config": config,
        "is_default": is_default,
    })
}

/// 載入有效的資料類別與其所有欄位定義（含已停用）。
async fn load_entity(
    conn: &mut PgConnection,
    key: &str,
) -> Result<(EntityDefinition, Vec<FieldDefinition>), CoreError> {
    let row = schema_db::find_entity_by_key(&mut *conn, key).await?;
    load_fields_of(conn, row).await
}

/// 同 [`load_entity`]，寫入時使用：以 `FOR SHARE` 鎖定資料類別，等待進行中的欄位定義變更完成。
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
        .filter(|e| e.is_active)
        .ok_or_else(|| CoreError::NotFound(ENTITY_NOT_FOUND.into()))?;
    let fields = schema_db::list_fields(&mut *conn, entity.id)
        .await?
        .into_iter()
        .map(field_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    Ok((entity, fields))
}

/// 鎖定版面並檢查是否可以修改：全域版面需要 `schema.manage`；別人的個人版面視為不存在。
async fn lock_layout(
    tx: &mut PgTransaction<'_>,
    ctx: &AuthContext,
    entity_id: Id,
    id: Id,
) -> Result<LayoutRow, CoreError> {
    let row = db::find_layout_for_update(&mut **tx, entity_id, id)
        .await?
        .ok_or_else(layout_not_found)?;
    match row.user_id {
        None => ctx.require(permission::SCHEMA_MANAGE)?,
        Some(owner) if owner == ctx.user_id => {}
        Some(_) => return Err(layout_not_found()),
    }
    Ok(row)
}

/// 取消範圍內 `exclude_id` 以外的預設版面，各寫入一筆修改紀錄。
async fn clear_other_defaults(
    tx: &mut PgTransaction<'_>,
    ctx: &AuthContext,
    entity_id: Id,
    user_id: Option<Id>,
    exclude_id: Id,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<(), CoreError> {
    let others =
        db::list_other_defaults_for_update(&mut **tx, entity_id, user_id, exclude_id).await?;
    for other in others {
        db::clear_default(&mut **tx, other.id, ctx.user_id).await?;
        record_audit(
            tx,
            ctx,
            action::UPDATE,
            other.id,
            AuditDiff {
                before: Some(json!({ "is_default": true })),
                after: Some(json!({ "is_default": false })),
            },
            client_info,
            now,
        )
        .await?;
    }
    Ok(())
}

/// 違反「預設版面只有一個」的唯一索引（同時有其他請求設定了預設）→ `Conflict`。
fn write_error(err: DbError) -> CoreError {
    match err.unique_violation() {
        Some(USER_DEFAULT_UNIQUE | GLOBAL_DEFAULT_UNIQUE) => {
            CoreError::Conflict(DEFAULT_TAKEN.into())
        }
        _ => err.into(),
    }
}

async fn record_audit(
    tx: &mut PgTransaction<'_>,
    ctx: &AuthContext,
    action: &str,
    layout_id: Id,
    diff: AuditDiff,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<(), CoreError> {
    audit::record(
        tx,
        &AuditEntry {
            user_id: Some(ctx.user_id),
            action,
            entity_key: Some(LAYOUT_AUDIT_KEY),
            record_id: Some(layout_id),
            diff,
            client_info,
            occurred_at: now,
        },
    )
    .await?;
    Ok(())
}

async fn fetch_layout(
    tx: &mut PgTransaction<'_>,
    entity_id: Id,
    id: Id,
    fields: &[FieldDefinition],
) -> Result<ViewLayout, CoreError> {
    let row = db::find_layout(&mut **tx, entity_id, id)
        .await?
        .ok_or_else(|| CoreError::Internal("寫入後找不到版面".into()))?;
    Ok(layout_from_row(row, fields))
}

fn layout_not_found() -> CoreError {
    CoreError::NotFound(LAYOUT_NOT_FOUND.into())
}

async fn commit(tx: PgTransaction<'_>) -> Result<(), CoreError> {
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hammer_shared::record::{FilterOp, RecordFilter, RecordSort};
    use hammer_shared::schema::{DataType, FieldStorage, SelectOption};

    fn field(key: &str, data_type: DataType, sort_order: i32) -> FieldDefinition {
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
            sort_order,
            is_active: true,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            version: 1,
        }
    }

    /// `name`(text)、`qty`(integer)、`tags`(multi_select)、`hidden`(text，不顯示)、
    /// `old`(text，已停用)。
    fn fields() -> Vec<FieldDefinition> {
        let mut hidden = field("hidden", DataType::Text, 4);
        hidden.is_visible = false;
        let mut old = field("old", DataType::Text, 0);
        old.is_active = false;
        let mut qty = field("qty", DataType::Integer, 2);
        qty.column_width = Some(80);
        vec![
            field("tags", DataType::MultiSelect, 3),
            qty,
            field("name", DataType::Text, 1),
            hidden,
            old,
        ]
    }

    fn column(key: &str, width: Option<i32>) -> LayoutColumn {
        LayoutColumn {
            field_key: key.into(),
            width,
        }
    }

    fn filter(key: &str, op: FilterOp, value: &str) -> RecordFilter {
        RecordFilter {
            field_key: key.into(),
            op,
            value: value.into(),
        }
    }

    fn sort(key: &str) -> Option<RecordSort> {
        Some(RecordSort {
            field_key: key.into(),
            descending: false,
        })
    }

    fn param_of(err: CoreError) -> String {
        match err {
            CoreError::InvalidParam { param, message } => {
                assert!(!message.is_empty());
                param
            }
            other => panic!("{other:?}"),
        }
    }

    fn validate(config: LayoutConfig) -> Result<LayoutConfig, CoreError> {
        validate_config(Id::nil(), &fields(), &config)
    }

    #[test]
    fn name_is_trimmed_and_required() {
        assert_eq!(validate_name("  我的版面 ").unwrap(), "我的版面");
        for bad in ["", "   "] {
            assert!(matches!(
                validate_name(bad),
                Err(CoreError::InvalidField { field: "name", .. })
            ));
        }
        let long = "字".repeat(MAX_LAYOUT_NAME_CHARS);
        assert!(validate_name(&long).is_ok());
        assert!(validate_name(&format!("{long}字")).is_err());
    }

    #[test]
    fn valid_config_passes() {
        let config = LayoutConfig {
            columns: vec![column("qty", Some(120)), column("hidden", None)],
            sort: sort("qty"),
            filters: vec![
                filter("qty", FilterOp::Gte, "10"),
                filter("tags", FilterOp::Any, "a"),
                filter("tags", FilterOp::Any, "b"),
            ],
        };
        assert_eq!(validate(config.clone()).unwrap(), config);
        assert!(validate(LayoutConfig::default()).is_ok());
    }

    #[test]
    fn invalid_columns_are_rejected() {
        let cases = [
            (
                vec![column("name", None), column("missing", None)],
                "config.columns[1].field_key",
            ),
            (vec![column("old", None)], "config.columns[0].field_key"),
            (
                vec![column("name", None), column("name", None)],
                "config.columns[1].field_key",
            ),
            (vec![column("name", Some(0))], "config.columns[0].width"),
        ];
        for (columns, param) in cases {
            let err = validate(LayoutConfig {
                columns,
                ..Default::default()
            })
            .unwrap_err();
            assert_eq!(param_of(err), param);
        }
    }

    #[test]
    fn invalid_sort_and_filters_are_rejected() {
        let err = validate(LayoutConfig {
            sort: sort("old"),
            ..Default::default()
        })
        .unwrap_err();
        assert_eq!(param_of(err), "config.sort");

        for bad in [
            filter("missing", FilterOp::Eq, "x"),
            filter("name", FilterOp::Gt, "x"),
            filter("qty", FilterOp::Eq, "abc"),
        ] {
            let err = validate(LayoutConfig {
                filters: vec![filter("name", FilterOp::Contains, "王"), bad],
                ..Default::default()
            })
            .unwrap_err();
            assert_eq!(param_of(err), "config.filters[1]");
        }
    }

    #[test]
    fn sanitize_drops_inactive_and_missing_fields() {
        let config = LayoutConfig {
            columns: vec![
                column("old", None),
                column("qty", Some(100)),
                column("missing", None),
                column("qty", Some(50)),
                column("name", None),
            ],
            sort: sort("old"),
            filters: vec![
                filter("missing", FilterOp::Eq, "x"),
                filter("qty", FilterOp::Gte, "1"),
                // 欄位型態變更後運算子不再適用
                filter("name", FilterOp::Gte, "1"),
                filter("old", FilterOp::Empty, "true"),
            ],
        };
        assert_eq!(
            sanitize_config(Id::nil(), &fields(), config),
            LayoutConfig {
                columns: vec![column("qty", Some(100)), column("name", None)],
                sort: None,
                filters: vec![filter("qty", FilterOp::Gte, "1")],
            }
        );
    }

    #[test]
    fn sanitize_keeps_valid_config() {
        let config = LayoutConfig {
            columns: vec![column("tags", None), column("name", Some(200))],
            sort: sort("name"),
            filters: vec![filter("tags", FilterOp::Any, "a")],
        };
        assert_eq!(
            sanitize_config(Id::nil(), &fields(), config.clone()),
            config
        );
    }

    #[test]
    fn generated_config_uses_visible_active_fields_in_order() {
        assert_eq!(
            generated_config(&fields()),
            LayoutConfig {
                columns: vec![
                    column("name", None),
                    column("qty", Some(80)),
                    column("tags", None),
                ],
                sort: None,
                filters: Vec::new(),
            }
        );
        assert_eq!(generated_config(&[]), LayoutConfig::default());
    }

    #[test]
    fn non_unique_db_errors_pass_through() {
        assert!(matches!(
            write_error(DbError::Conflict),
            CoreError::Db(DbError::Conflict)
        ));
    }
}

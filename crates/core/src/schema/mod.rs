//! 資料類別與欄位定義的管理（docs/architecture.md §6.1、§7.1、§7.2、§8.5、§9.4）。
//!
//! - 查詢（需要登入，不需特定權限）：[`list_entities`]、[`get_entity`]
//! - 寫入（需要 `schema.manage`）：[`create_entity`]、[`update_entity`]、[`create_field`]、
//!   [`update_field`]、[`deactivate_field`]、[`reorder_fields`]、[`change_field_type`]
//!
//! 寫入都在單一 transaction 內完成：先鎖定資料類別（`FOR UPDATE`），讓同一個資料類別的
//! 欄位變更依序進行；檢查 `version`（§9.4）；每個有變動的定義寫入一筆 `schema_change_logs`
//! （完整的 before / after）與一筆 `audit_logs`（只有變動的屬性）。沒有任何變動時不寫入。
//!
//! 欄位的「可搜尋」、「不可重複」與啟用狀態改變時，在同一個 transaction 內建立或移除動態索引
//! （§7.5，[`index`]）。
//!
//! 輸入檢查在 [`rules`]，型態轉換在 [`type_change`]，索引規劃在 [`index`]（純邏輯，有單元測試）。

pub mod index;
pub mod rules;
pub mod type_change;

use hammer_db::dynamic_index::{self, ExpressionIndex};
use hammer_db::record as record_db;
use hammer_db::schema::{
    self as db, ENTITY_KEY_UNIQUE, EntityRow, EntityUpdate, FIELD_KEY_UNIQUE, FieldAttributes,
    FieldRow, FieldTypeUpdate, NewEntity, NewField, NewSchemaChangeLog,
};
use hammer_db::sqlx::{PgPool, PgTransaction};
use hammer_db::{DbError, ensure_version_matched};
use hammer_shared::schema::{
    ChangeFieldTypeRequest, CreateEntityRequest, CreateFieldRequest, DataType, EntityDefinition,
    EntityDetail, EntityKind, EntityStorage, FieldDefinition, FieldStorage,
    MAX_CONVERSION_FAILURES, MAX_DUPLICATE_VALUES, ReorderFieldsRequest, UpdateEntityRequest,
    UpdateFieldRequest,
};
use hammer_shared::{Id, audit as action, new_id, permission};
use serde::Serialize;
use serde_json::Value;
use time::OffsetDateTime;

use crate::CoreError;
use crate::audit::{self, AuditDiff, AuditEntry};
use crate::auth::AuthContext;
use index::{DuplicateValue, IndexChange, IndexKind};
use rules::field;
use type_change::StoredRecord;

/// 資料類別定義在 `audit_logs.entity_key` 的值。
pub const ENTITY_DEFINITION_KEY: &str = "entity_definition";
/// 欄位定義在 `audit_logs.entity_key` 的值。
pub const FIELD_DEFINITION_KEY: &str = "field_definition";

/// 資料類別不存在或已刪除時的錯誤訊息。
pub const ENTITY_NOT_FOUND: &str = "找不到此資料類別";
/// 欄位不存在或已刪除時的錯誤訊息。
pub const FIELD_NOT_FOUND: &str = "找不到此欄位";
/// 資料類別代碼重複時的錯誤訊息。
pub const ENTITY_KEY_TAKEN: &str = "此資料類別代碼已被使用";
/// 欄位代碼重複時的錯誤訊息（含已停用的欄位）。
pub const FIELD_KEY_TAKEN: &str = "此欄位代碼已被使用";

/// `schema_change_logs.target_type`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeTarget {
    Entity,
    Field,
}

impl ChangeTarget {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Entity => "entity",
            Self::Field => "field",
        }
    }

    fn audit_entity_key(self) -> &'static str {
        match self {
            Self::Entity => ENTITY_DEFINITION_KEY,
            Self::Field => FIELD_DEFINITION_KEY,
        }
    }
}

/// `schema_change_logs.action`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeAction {
    Create,
    Update,
    Deactivate,
}

impl ChangeAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::Deactivate => "deactivate",
        }
    }

    /// 對應的 `audit_logs.action`。停用屬於修改。
    pub fn audit_action(self) -> &'static str {
        match self {
            Self::Create => action::CREATE,
            Self::Update | Self::Deactivate => action::UPDATE,
        }
    }

    /// 由啟用狀態的變化決定：啟用 → 停用為 `Deactivate`，其他為 `Update`。
    pub fn for_update(was_active: bool, is_active: bool) -> Self {
        if was_active && !is_active {
            Self::Deactivate
        } else {
            Self::Update
        }
    }
}

// ---------------------------------------------------------------------------
// 資料列轉換
// ---------------------------------------------------------------------------

/// 資料列轉成 API 回應。列舉值不合法時回傳 [`CoreError::Internal`]。
pub fn entity_from_row(row: EntityRow) -> Result<EntityDefinition, CoreError> {
    Ok(EntityDefinition {
        kind: EntityKind::parse(&row.kind)
            .ok_or_else(|| corrupted("entity_definitions.kind", &row.kind))?,
        storage: EntityStorage::parse(&row.storage)
            .ok_or_else(|| corrupted("entity_definitions.storage", &row.storage))?,
        id: row.id,
        key: row.key,
        label: row.label,
        icon: row.icon,
        is_system: row.is_system,
        parent_entity_id: row.parent_entity_id,
        numbering_pattern: row.numbering_pattern,
        title_template: row.title_template,
        sort_order: row.sort_order,
        is_active: row.is_active,
        created_at: row.created_at,
        updated_at: row.updated_at,
        version: row.version,
    })
}

/// 資料列轉成 API 回應。列舉值或 JSONB 格式不合法時回傳 [`CoreError::Internal`]。
pub fn field_from_row(row: FieldRow) -> Result<FieldDefinition, CoreError> {
    let formula_result_type = row
        .formula_result_type
        .as_deref()
        .map(|t| {
            DataType::parse(t).ok_or_else(|| corrupted("field_definitions.formula_result_type", t))
        })
        .transpose()?;
    Ok(FieldDefinition {
        data_type: DataType::parse(&row.data_type)
            .ok_or_else(|| corrupted("field_definitions.data_type", &row.data_type))?,
        storage: FieldStorage::parse(&row.storage)
            .ok_or_else(|| corrupted("field_definitions.storage", &row.storage))?,
        validation: from_json(row.validation, "field_definitions.validation")?,
        options: from_json(row.options, "field_definitions.options")?,
        formula_result_type,
        id: row.id,
        entity_id: row.entity_id,
        field_key: row.field_key,
        label: row.label,
        is_system: row.is_system,
        is_required: row.is_required,
        is_visible: row.is_visible,
        is_readonly: row.is_readonly,
        is_searchable: row.is_searchable,
        is_unique: row.is_unique,
        default_value: row.default_value,
        decimal_places: row.decimal_places,
        relation_entity_id: row.relation_entity_id,
        formula: row.formula,
        ui_widget: row.ui_widget,
        column_width: row.column_width,
        help_text: row.help_text,
        sort_order: row.sort_order,
        is_active: row.is_active,
        created_at: row.created_at,
        updated_at: row.updated_at,
        version: row.version,
    })
}

fn from_json<T: serde::de::DeserializeOwned>(
    value: Option<Value>,
    column: &str,
) -> Result<Option<T>, CoreError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(v) => serde_json::from_value(v)
            .map(Some)
            .map_err(|e| CoreError::Internal(format!("{column} 格式錯誤：{e}"))),
    }
}

fn corrupted(column: &str, value: &str) -> CoreError {
    CoreError::Internal(format!("{column} 的值不合法：{value}"))
}

/// 寫入變更紀錄用的快照：定義的所有屬性，不含 `created_at`、`updated_at`、`version`。
pub fn snapshot(value: &impl Serialize) -> Result<Value, CoreError> {
    let mut json = serde_json::to_value(value)
        .map_err(|e| CoreError::Internal(format!("序列化定義失敗：{e}")))?;
    if let Some(map) = json.as_object_mut() {
        for key in ["created_at", "updated_at", "version"] {
            map.remove(key);
        }
    }
    Ok(json)
}

/// 欄位定義的可設定屬性轉成 JSONB 參數。
struct FieldJson {
    validation: Option<Value>,
    options: Option<Value>,
}

impl FieldJson {
    fn of(def: &FieldDefinition) -> Result<Self, CoreError> {
        let to_json = |v: Result<Value, serde_json::Error>| {
            v.map_err(|e| CoreError::Internal(format!("序列化欄位設定失敗：{e}")))
        };
        Ok(Self {
            validation: def
                .validation
                .as_ref()
                .map(|v| to_json(serde_json::to_value(v)))
                .transpose()?,
            options: def
                .options
                .as_ref()
                .map(|v| to_json(serde_json::to_value(v)))
                .transpose()?,
        })
    }

    fn attrs<'a>(&'a self, def: &'a FieldDefinition) -> FieldAttributes<'a> {
        FieldAttributes {
            label: &def.label,
            is_required: def.is_required,
            is_visible: def.is_visible,
            is_readonly: def.is_readonly,
            is_searchable: def.is_searchable,
            is_unique: def.is_unique,
            default_value: def.default_value.as_ref(),
            validation: self.validation.as_ref(),
            options: self.options.as_ref(),
            decimal_places: def.decimal_places,
            ui_widget: def.ui_widget.as_deref(),
            column_width: def.column_width,
            help_text: def.help_text.as_deref(),
            sort_order: def.sort_order,
        }
    }
}

// ---------------------------------------------------------------------------
// 查詢
// ---------------------------------------------------------------------------

/// 列出有效（啟用、未刪除）的資料類別，依 `sort_order` 排序。需要登入。
pub async fn list_entities(
    pool: &PgPool,
    _ctx: &AuthContext,
) -> Result<Vec<EntityDefinition>, CoreError> {
    db::list_active_entities(pool)
        .await?
        .into_iter()
        .map(entity_from_row)
        .collect()
}

/// 取得資料類別（含已停用）與其所有欄位定義（含已停用），欄位依 `sort_order` 排序。需要登入。
pub async fn get_entity(
    pool: &PgPool,
    _ctx: &AuthContext,
    key: &str,
) -> Result<EntityDetail, CoreError> {
    let mut conn = pool.acquire().await.map_err(DbError::from)?;
    let row = db::find_entity_by_key(&mut *conn, key)
        .await?
        .ok_or_else(entity_not_found)?;
    let fields = load_fields(&mut conn, row.id).await?;
    Ok(EntityDetail {
        entity: entity_from_row(row)?,
        fields,
    })
}

// ---------------------------------------------------------------------------
// 資料類別
// ---------------------------------------------------------------------------

/// 建立自訂資料類別（`kind = master`、`storage = custom_records`）。
///
/// 代碼與既有資料類別（含已刪除）重複 → [`CoreError::InvalidField`]（`key`）。
pub async fn create_entity(
    pool: &PgPool,
    ctx: &AuthContext,
    req: &CreateEntityRequest,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<EntityDetail, CoreError> {
    ctx.require(permission::SCHEMA_MANAGE)?;
    let plan = rules::plan_create_entity(req)?;

    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let id = new_id();
    db::insert_entity(
        &mut *tx,
        &NewEntity {
            id,
            key: &plan.key,
            label: &plan.label,
            icon: plan.icon.as_deref(),
            kind: plan.kind.as_str(),
            storage: plan.storage.as_str(),
            numbering_pattern: plan.numbering_pattern.as_deref(),
            title_template: plan.title_template.as_deref(),
            sort_order: plan.sort_order,
            created_by: ctx.user_id,
            created_at: now,
        },
    )
    .await
    .map_err(|e| {
        if e.unique_violation() == Some(ENTITY_KEY_UNIQUE) {
            invalid(field::KEY, ENTITY_KEY_TAKEN)
        } else {
            e.into()
        }
    })?;
    let entity = fetch_entity(&mut tx, &plan.key).await?;
    log_change(
        &mut tx,
        ctx,
        ChangeTarget::Entity,
        id,
        ChangeAction::Create,
        None,
        Some(&snapshot(&entity)?),
        client_info,
        now,
    )
    .await?;
    commit(tx).await?;

    tracing::info!(actor = %ctx.user_id, entity = %plan.key, "已建立資料類別");
    Ok(EntityDetail {
        entity,
        fields: Vec::new(),
    })
}

/// 修改資料類別的顯示名稱、圖示、單號規則、標題範本、排序、是否啟用。
///
/// 標題範本只能引用此資料類別已定義的欄位（含已停用）或 `{record_no}`（[`crate::title`]）。
///
/// - 不存在或已刪除 → [`CoreError::NotFound`]
/// - `version` 不符 → `Conflict`（§9.4）
/// - 修改代碼、停用系統資料類別 → [`CoreError::InvalidField`]
/// - 沒有任何變動時不寫入，回傳目前的資料（`version` 不變）
pub async fn update_entity(
    pool: &PgPool,
    ctx: &AuthContext,
    key: &str,
    req: &UpdateEntityRequest,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<EntityDetail, CoreError> {
    ctx.require(permission::SCHEMA_MANAGE)?;

    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let current = lock_entity(&mut tx, key).await?;
    ensure_version(current.version, req.version)?;
    let fields = load_fields(&mut tx, current.id).await?;
    let next = rules::plan_update_entity(&current, &fields, req)?;

    let before = snapshot(&current)?;
    let after = snapshot(&next)?;
    let entity = if before == after {
        current
    } else {
        ensure_version_matched(
            db::update_entity(
                &mut *tx,
                &EntityUpdate {
                    id: current.id,
                    version: req.version,
                    label: &next.label,
                    icon: next.icon.as_deref(),
                    numbering_pattern: next.numbering_pattern.as_deref(),
                    title_template: next.title_template.as_deref(),
                    sort_order: next.sort_order,
                    is_active: next.is_active,
                    updated_by: ctx.user_id,
                },
            )
            .await?,
        )?;
        log_change(
            &mut tx,
            ctx,
            ChangeTarget::Entity,
            current.id,
            ChangeAction::for_update(current.is_active, next.is_active),
            Some(&before),
            Some(&after),
            client_info,
            now,
        )
        .await?;
        tracing::info!(actor = %ctx.user_id, entity = %key, "已修改資料類別");
        fetch_entity(&mut tx, key).await?
    };
    commit(tx).await?;
    Ok(EntityDetail { entity, fields })
}

// ---------------------------------------------------------------------------
// 欄位
// ---------------------------------------------------------------------------

/// 新增自訂欄位（`storage = json`）。設定必須與型態相符（[`rules::validate_field_config`]）。
/// 「可搜尋」、「不可重複」時建立對應的索引（§7.5）。
///
/// - 資料類別不存在或已刪除 → [`CoreError::NotFound`]
/// - 欄位代碼與該資料類別的既有欄位（含已停用）重複 → [`CoreError::InvalidField`]（`field_key`）
pub async fn create_field(
    pool: &PgPool,
    ctx: &AuthContext,
    entity_key: &str,
    req: &CreateFieldRequest,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<FieldDefinition, CoreError> {
    ctx.require(permission::SCHEMA_MANAGE)?;

    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let entity = lock_entity(&mut tx, entity_key).await?;
    let def = rules::plan_create_field(entity.id, req, now)?;
    let json = FieldJson::of(&def)?;
    db::insert_field(
        &mut *tx,
        &NewField {
            id: def.id,
            entity_id: entity.id,
            field_key: &def.field_key,
            data_type: def.data_type.as_str(),
            attrs: json.attrs(&def),
            created_by: ctx.user_id,
            created_at: now,
        },
    )
    .await
    .map_err(|e| {
        if e.unique_violation() == Some(FIELD_KEY_UNIQUE) {
            invalid(field::FIELD_KEY, FIELD_KEY_TAKEN)
        } else {
            e.into()
        }
    })?;
    let created = fetch_field(&mut tx, entity.id, &def.field_key).await?;
    sync_indexes(&mut tx, &entity, None, &created).await?;
    log_change(
        &mut tx,
        ctx,
        ChangeTarget::Field,
        created.id,
        ChangeAction::Create,
        None,
        Some(&snapshot(&created)?),
        client_info,
        now,
    )
    .await?;
    commit(tx).await?;

    tracing::info!(actor = %ctx.user_id, entity = %entity_key, field = %def.field_key, "已新增欄位");
    Ok(created)
}

/// 修改欄位屬性（整筆取代），不包含資料型態。
///
/// - 資料類別或欄位不存在 → [`CoreError::NotFound`]
/// - `version` 不符 → `Conflict`（§9.4）
/// - 修改代碼、型態，或違反系統欄位的限制（§7.2）→ [`CoreError::InvalidField`]
/// - `is_active` 由 `true` 改為 `false` 時，變更紀錄的動作為 `deactivate`
/// - 「可搜尋」、「不可重複」、啟用狀態改變時建立或移除索引（§7.5）；開啟「不可重複」
///   （或重新啟用不可重複的欄位）時，未刪除的資料已有重複的值 → [`CoreError::DuplicateValues`]
/// - 沒有任何變動時不寫入，回傳目前的資料（`version` 不變）
pub async fn update_field(
    pool: &PgPool,
    ctx: &AuthContext,
    entity_key: &str,
    field_key: &str,
    req: &UpdateFieldRequest,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<FieldDefinition, CoreError> {
    ctx.require(permission::SCHEMA_MANAGE)?;

    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let entity = lock_entity(&mut tx, entity_key).await?;
    let current = lock_field(&mut tx, entity.id, field_key).await?;
    ensure_version(current.version, req.version)?;
    let next = rules::plan_update_field(&current, req)?;
    let result = write_field(
        &mut tx,
        ctx,
        &entity,
        &current,
        &next,
        req.version,
        client_info,
        now,
    )
    .await?;
    commit(tx).await?;
    Ok(result)
}

/// 停用欄位（`is_active = false`），資料保留（§7.2），並移除欄位的索引（§7.5）。
///
/// - 系統欄位 → [`CoreError::InvalidField`]（`is_active`）
/// - 已經停用時不寫入，回傳目前的資料
/// - 其他錯誤同 [`update_field`]
pub async fn deactivate_field(
    pool: &PgPool,
    ctx: &AuthContext,
    entity_key: &str,
    field_key: &str,
    version: i32,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<FieldDefinition, CoreError> {
    ctx.require(permission::SCHEMA_MANAGE)?;

    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let entity = lock_entity(&mut tx, entity_key).await?;
    let current = lock_field(&mut tx, entity.id, field_key).await?;
    ensure_version(current.version, version)?;
    if current.is_system {
        return Err(invalid(field::IS_ACTIVE, "系統欄位不可停用"));
    }
    let next = FieldDefinition {
        is_active: false,
        ..current.clone()
    };
    let result = write_field(
        &mut tx,
        ctx,
        &entity,
        &current,
        &next,
        version,
        client_info,
        now,
    )
    .await?;
    commit(tx).await?;
    Ok(result)
}

/// 調整欄位排序。輸入必須依新的順序列出所有欄位（含已停用）各一次（[`rules::plan_reorder`]）。
///
/// - 任一欄位的 `version` 不符 → `Conflict`，整批不寫入
/// - 只有排序改變的欄位會更新與留紀錄
///
/// 回傳所有欄位，依新的排序。
pub async fn reorder_fields(
    pool: &PgPool,
    ctx: &AuthContext,
    entity_key: &str,
    req: &ReorderFieldsRequest,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<Vec<FieldDefinition>, CoreError> {
    ctx.require(permission::SCHEMA_MANAGE)?;

    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let entity = lock_entity(&mut tx, entity_key).await?;
    let current = db::list_fields_for_update(&mut *tx, entity.id)
        .await?
        .into_iter()
        .map(field_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    let steps = rules::plan_reorder(&current, &req.fields)?;
    for step in &steps {
        ensure_version(step.field.version, step.version)?;
    }
    for step in steps.iter().filter(|s| s.changes()) {
        ensure_version_matched(
            db::update_field_sort_order(
                &mut *tx,
                step.field.id,
                step.version,
                step.sort_order,
                ctx.user_id,
            )
            .await?,
        )?;
        let next = FieldDefinition {
            sort_order: step.sort_order,
            ..step.field.clone()
        };
        log_change(
            &mut tx,
            ctx,
            ChangeTarget::Field,
            step.field.id,
            ChangeAction::Update,
            Some(&snapshot(&step.field)?),
            Some(&snapshot(&next)?),
            client_info,
            now,
        )
        .await?;
    }
    let fields = load_fields(&mut tx, entity.id).await?;
    commit(tx).await?;

    tracing::info!(actor = %ctx.user_id, entity = %entity_key, "已調整欄位排序");
    Ok(fields)
}

/// 變更自訂欄位的資料型態（§7.4）。
///
/// 在同一個 transaction 內：
///
/// 1. 鎖定資料類別（`FOR UPDATE`）。寫入資料時以 `FOR SHARE` 鎖定資料類別
///    （[`crate::records`]），所以會等進行中的寫入完成，轉換期間也不會有新的寫入
/// 2. 檢查 `version` 與新的設定（[`rules::plan_change_type`]）
/// 3. 轉換該資料類別的所有資料（含已軟刪除，[`type_change::convert_records`]）；
///    有任何一筆失敗 → [`CoreError::ConversionFailed`]，不修改任何資料
/// 4. 寫入轉換後有變動的資料（遞增各筆的 `version`）與新的欄位定義
/// 5. 寫入一筆 `schema_change_logs`（`update`，完整的 before / after）與欄位定義的 `audit_logs`
///
/// 動態索引（§7.5）的運算式是 `data->>'field_key'`，與型態無關，PostgreSQL 在更新資料時
/// 一併維護，不需重建。「不可重複」的欄位在第 3 步先檢查轉換後的值不重複。
///
/// 錯誤：資料類別或欄位不存在 → [`CoreError::NotFound`]；`version` 不符 → `Conflict`；
/// 系統欄位、型態相同或不支援、設定不合法 → [`CoreError::InvalidField`]。
pub async fn change_field_type(
    pool: &PgPool,
    ctx: &AuthContext,
    entity_key: &str,
    field_key: &str,
    req: &ChangeFieldTypeRequest,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<FieldDefinition, CoreError> {
    ctx.require(permission::SCHEMA_MANAGE)?;

    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let entity = lock_entity(&mut tx, entity_key).await?;
    let current = lock_field(&mut tx, entity.id, field_key).await?;
    ensure_version(current.version, req.version)?;
    let (next, _) = rules::plan_change_type(&current, req)?;

    let rows = if entity.storage == EntityStorage::CustomRecords {
        record_db::list_all_record_data_for_update(&mut *tx, entity.id).await?
    } else {
        // 系統資料表的 custom_data 在 M3 支援；目前系統資料類別沒有自訂欄位
        return Err(invalid(
            field::DATA_TYPE,
            "系統資料類別的欄位目前不能變更資料型態",
        ));
    };
    let records = rows
        .iter()
        .map(|row| {
            Ok(StoredRecord {
                id: row.id,
                data: row.data.as_object().ok_or_else(|| {
                    CoreError::Internal("custom_records.data 不是 JSON 物件".into())
                })?,
                deleted: row.deleted,
            })
        })
        .collect::<Result<Vec<_>, CoreError>>()?;
    let converted = match type_change::convert_records(&current, &next, &records) {
        Ok(converted) => converted,
        Err(mut failures) => {
            let total = failures.len();
            failures.truncate(MAX_CONVERSION_FAILURES);
            return Err(CoreError::ConversionFailed { failures, total });
        }
    };

    let (ids, data): (Vec<Id>, Vec<Value>) = converted
        .into_iter()
        .map(|r| (r.id, Value::Object(r.data)))
        .unzip();
    if !ids.is_empty() {
        let updated =
            record_db::update_records_data(&mut *tx, entity.id, &ids, &data, ctx.user_id).await?;
        if updated != ids.len() as u64 {
            return Err(CoreError::Internal(format!(
                "轉換資料時應更新 {} 筆，實際更新 {updated} 筆",
                ids.len()
            )));
        }
    }

    let json = FieldJson::of(&next)?;
    ensure_version_matched(
        db::update_field_type(
            &mut *tx,
            &FieldTypeUpdate {
                id: current.id,
                version: req.version,
                data_type: next.data_type.as_str(),
                decimal_places: next.decimal_places,
                options: json.options.as_ref(),
                validation: json.validation.as_ref(),
                default_value: next.default_value.as_ref(),
                updated_by: ctx.user_id,
            },
        )
        .await?,
    )?;
    log_change(
        &mut tx,
        ctx,
        ChangeTarget::Field,
        current.id,
        ChangeAction::Update,
        Some(&snapshot(&current)?),
        Some(&snapshot(&next)?),
        client_info,
        now,
    )
    .await?;
    let result = fetch_field(&mut tx, entity.id, field_key).await?;
    commit(tx).await?;

    tracing::info!(
        actor = %ctx.user_id,
        entity = %entity_key,
        field = %field_key,
        from = current.data_type.as_str(),
        to = next.data_type.as_str(),
        converted = ids.len(),
        "已變更欄位資料型態"
    );
    Ok(result)
}

/// 寫入欄位的修改、同步索引並留紀錄。沒有變動時回傳 `current`。
#[allow(clippy::too_many_arguments)]
async fn write_field(
    tx: &mut PgTransaction<'_>,
    ctx: &AuthContext,
    entity: &EntityDefinition,
    current: &FieldDefinition,
    next: &FieldDefinition,
    version: i32,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<FieldDefinition, CoreError> {
    let before = snapshot(current)?;
    let after = snapshot(next)?;
    if before == after {
        return Ok(current.clone());
    }
    let json = FieldJson::of(next)?;
    ensure_version_matched(
        db::update_field(
            &mut **tx,
            current.id,
            version,
            &json.attrs(next),
            next.is_active,
            ctx.user_id,
        )
        .await?,
    )?;
    sync_indexes(tx, entity, Some(current), next).await?;
    log_change(
        tx,
        ctx,
        ChangeTarget::Field,
        current.id,
        ChangeAction::for_update(current.is_active, next.is_active),
        Some(&before),
        Some(&after),
        client_info,
        now,
    )
    .await?;
    tracing::info!(actor = %ctx.user_id, field_id = %current.id, field = %current.field_key, "已修改欄位");
    fetch_field(tx, current.entity_id, &current.field_key).await
}

/// 依欄位定義的變化建立或移除動態索引（§7.5，[`index::plan_index_changes`]）。
///
/// 建立唯一索引前先檢查未刪除的資料，有重複的值 → [`CoreError::DuplicateValues`]。
/// 呼叫端已用 `FOR UPDATE` 鎖定資料類別，寫入資料時以 `FOR SHARE` 鎖定（[`crate::records`]），
/// 所以檢查與建立之間不會有新的資料寫入。
///
/// 系統資料類別（`storage = table`）的 `custom_data` 索引在 M3 處理，目前不建立。
async fn sync_indexes(
    tx: &mut PgTransaction<'_>,
    entity: &EntityDefinition,
    current: Option<&FieldDefinition>,
    next: &FieldDefinition,
) -> Result<(), CoreError> {
    let changes = index::plan_index_changes(current, next);
    if changes.is_empty() || entity.storage != EntityStorage::CustomRecords {
        return Ok(());
    }
    for change in changes {
        match change {
            IndexChange::Drop(kind) => {
                let name = index::index_name(kind, &entity.key, &next.field_key)?;
                dynamic_index::drop_index(&mut **tx, &name).await?;
                tracing::info!(entity = %entity.key, field = %next.field_key, index = %name, "已移除欄位索引");
            }
            IndexChange::Create(kind) => {
                let unique = kind == IndexKind::Unique;
                if unique {
                    ensure_no_duplicates(tx, entity.id, &next.field_key).await?;
                }
                let name = index::index_name(kind, &entity.key, &next.field_key)?;
                dynamic_index::create_index(
                    &mut **tx,
                    &ExpressionIndex {
                        name: &name,
                        entity_id: entity.id,
                        field_key: &next.field_key,
                        unique,
                    },
                )
                .await?;
                tracing::info!(entity = %entity.key, field = %next.field_key, index = %name, "已建立欄位索引");
            }
        }
    }
    Ok(())
}

/// 未刪除的資料中，欄位值不可重複。有重複 → [`CoreError::DuplicateValues`]。
async fn ensure_no_duplicates(
    tx: &mut PgTransaction<'_>,
    entity_id: Id,
    field_key: &str,
) -> Result<(), CoreError> {
    // 常數很小，不會溢位
    let limit = MAX_DUPLICATE_VALUES as i32;
    let rows = dynamic_index::find_duplicate_values(&mut **tx, entity_id, field_key, limit).await?;
    let Some(first) = rows.first() else {
        return Ok(());
    };
    let total = usize::try_from(first.total)
        .map_err(|_| CoreError::Internal(format!("重複值的個數不正確：{}", first.total)))?;
    let duplicates = rows
        .into_iter()
        .map(|r| DuplicateValue {
            value: r.value,
            count: r.count,
            record_ids: r.record_ids,
        })
        .collect();
    Err(CoreError::DuplicateValues { duplicates, total })
}

// ---------------------------------------------------------------------------
// 共用
// ---------------------------------------------------------------------------

/// 寫入一筆 `schema_change_logs`（完整快照）與一筆 `audit_logs`（只有變動的屬性）。
#[allow(clippy::too_many_arguments)]
async fn log_change(
    tx: &mut PgTransaction<'_>,
    ctx: &AuthContext,
    target: ChangeTarget,
    target_id: Id,
    change: ChangeAction,
    before: Option<&Value>,
    after: Option<&Value>,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<(), CoreError> {
    db::insert_schema_change_log(
        &mut **tx,
        &NewSchemaChangeLog {
            id: new_id(),
            changed_at: now,
            changed_by: ctx.user_id,
            target_type: target.as_str(),
            target_id,
            action: change.as_str(),
            before_data: before,
            after_data: after,
        },
    )
    .await?;
    audit::record(
        tx,
        &AuditEntry {
            user_id: Some(ctx.user_id),
            action: change.audit_action(),
            entity_key: Some(target.audit_entity_key()),
            record_id: Some(target_id),
            diff: AuditDiff::between(before, after)?,
            client_info,
            occurred_at: now,
        },
    )
    .await?;
    Ok(())
}

async fn lock_entity(tx: &mut PgTransaction<'_>, key: &str) -> Result<EntityDefinition, CoreError> {
    let row = db::find_entity_by_key_for_update(&mut **tx, key)
        .await?
        .ok_or_else(entity_not_found)?;
    entity_from_row(row)
}

async fn lock_field(
    tx: &mut PgTransaction<'_>,
    entity_id: Id,
    field_key: &str,
) -> Result<FieldDefinition, CoreError> {
    let row = db::find_field_for_update(&mut **tx, entity_id, field_key)
        .await?
        .ok_or_else(|| CoreError::NotFound(FIELD_NOT_FOUND.into()))?;
    field_from_row(row)
}

async fn fetch_entity(
    tx: &mut PgTransaction<'_>,
    key: &str,
) -> Result<EntityDefinition, CoreError> {
    let row = db::find_entity_by_key(&mut **tx, key)
        .await?
        .ok_or_else(|| CoreError::Internal("寫入後找不到資料類別".into()))?;
    entity_from_row(row)
}

async fn fetch_field(
    tx: &mut PgTransaction<'_>,
    entity_id: Id,
    field_key: &str,
) -> Result<FieldDefinition, CoreError> {
    let row = db::find_field(&mut **tx, entity_id, field_key)
        .await?
        .ok_or_else(|| CoreError::Internal("寫入後找不到欄位".into()))?;
    field_from_row(row)
}

async fn load_fields(
    conn: &mut hammer_db::sqlx::PgConnection,
    entity_id: Id,
) -> Result<Vec<FieldDefinition>, CoreError> {
    db::list_fields(&mut *conn, entity_id)
        .await?
        .into_iter()
        .map(field_from_row)
        .collect()
}

/// 版本號不符時回傳 `Conflict`（§9.4）。
pub(crate) fn ensure_version(current: i32, expected: i32) -> Result<(), CoreError> {
    if current == expected {
        Ok(())
    } else {
        Err(DbError::Conflict.into())
    }
}

fn entity_not_found() -> CoreError {
    CoreError::NotFound(ENTITY_NOT_FOUND.into())
}

fn invalid(field: &'static str, message: &str) -> CoreError {
    CoreError::InvalidField {
        field,
        message: message.to_owned(),
    }
}

async fn commit(tx: PgTransaction<'_>) -> Result<(), CoreError> {
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hammer_shared::ErrorCode;
    use serde_json::json;
    use time::macros::datetime;

    const NOW: OffsetDateTime = datetime!(2026-11-13 09:00 +8);

    fn entity_row() -> EntityRow {
        EntityRow {
            id: new_id(),
            key: "repair_order".into(),
            label: "維修單".into(),
            icon: None,
            kind: "master".into(),
            is_system: false,
            storage: "custom_records".into(),
            table_name: None,
            parent_entity_id: None,
            numbering_pattern: None,
            title_template: None,
            sort_order: 0,
            is_active: true,
            created_at: NOW,
            updated_at: NOW,
            version: 1,
        }
    }

    fn field_row() -> FieldRow {
        FieldRow {
            id: new_id(),
            entity_id: new_id(),
            field_key: "status".into(),
            label: "狀態".into(),
            data_type: "select".into(),
            is_system: false,
            storage: "json".into(),
            column_name: None,
            is_required: false,
            is_visible: true,
            is_readonly: false,
            is_searchable: false,
            is_unique: false,
            default_value: Some(json!("open")),
            validation: Some(json!({ "max_length": 5 })),
            options: Some(json!([{ "value": "open", "label": "處理中", "color": null }])),
            decimal_places: None,
            relation_entity_id: None,
            formula: None,
            formula_result_type: None,
            ui_widget: None,
            column_width: Some(120),
            help_text: None,
            sort_order: 2,
            is_active: true,
            created_at: NOW,
            updated_at: NOW,
            version: 4,
        }
    }

    #[test]
    fn converts_entity_row() {
        let row = entity_row();
        let e = entity_from_row(row.clone()).unwrap();
        assert_eq!(e.id, row.id);
        assert_eq!(e.kind, EntityKind::Master);
        assert_eq!(e.storage, EntityStorage::CustomRecords);

        let bad = EntityRow {
            kind: "weird".into(),
            ..row
        };
        assert_eq!(
            entity_from_row(bad).unwrap_err().code(),
            ErrorCode::Internal
        );
    }

    #[test]
    fn converts_field_row() {
        let f = field_from_row(field_row()).unwrap();
        assert_eq!(f.data_type, DataType::Select);
        assert_eq!(f.storage, FieldStorage::Json);
        assert_eq!(f.validation.unwrap().max_length, Some(5));
        assert_eq!(f.options.unwrap()[0].value, "open");
        assert_eq!(f.version, 4);

        for bad in [
            FieldRow {
                data_type: "float".into(),
                ..field_row()
            },
            FieldRow {
                options: Some(json!("not an array")),
                ..field_row()
            },
            FieldRow {
                formula_result_type: Some("x".into()),
                ..field_row()
            },
        ] {
            assert_eq!(field_from_row(bad).unwrap_err().code(), ErrorCode::Internal);
        }
        let f = field_from_row(FieldRow {
            validation: Some(Value::Null),
            ..field_row()
        })
        .unwrap();
        assert_eq!(f.validation, None);
    }

    #[test]
    fn snapshot_drops_bookkeeping_fields() {
        let f = field_from_row(field_row()).unwrap();
        let s = snapshot(&f).unwrap();
        assert!(s.get("version").is_none());
        assert!(s.get("updated_at").is_none());
        assert!(s.get("created_at").is_none());
        assert_eq!(s["field_key"], json!("status"));
        // 只差在 version 時快照相同，視為沒有變動
        let bumped = FieldDefinition {
            version: 9,
            ..f.clone()
        };
        assert_eq!(snapshot(&bumped).unwrap(), s);
    }

    #[test]
    fn change_actions() {
        assert_eq!(
            ChangeAction::for_update(true, false),
            ChangeAction::Deactivate
        );
        assert_eq!(ChangeAction::for_update(true, true), ChangeAction::Update);
        assert_eq!(ChangeAction::for_update(false, true), ChangeAction::Update);
        assert_eq!(ChangeAction::Deactivate.audit_action(), action::UPDATE);
        assert_eq!(ChangeAction::Create.audit_action(), action::CREATE);
        for a in [
            ChangeAction::Create,
            ChangeAction::Update,
            ChangeAction::Deactivate,
        ] {
            assert!(action::is_valid(a.audit_action()));
        }
        assert_eq!(ChangeTarget::Field.as_str(), "field");
        assert_eq!(
            ChangeTarget::Entity.audit_entity_key(),
            ENTITY_DEFINITION_KEY
        );
    }

    #[test]
    fn version_check() {
        assert!(ensure_version(2, 2).is_ok());
        assert_eq!(
            ensure_version(2, 1).unwrap_err().code(),
            ErrorCode::Conflict
        );
    }
}

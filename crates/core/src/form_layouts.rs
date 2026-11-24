//! 表單版面（docs/architecture.md §7.7、§8.5 `form_layouts`、§9.4、§10）。
//!
//! - 查詢（需要 `record.read`）：[`get_form_layout`]
//! - 寫入（需要 `schema.manage`）：[`save_form_layout`]、[`reset_form_layout`]
//!
//! 只處理有效（啟用、未刪除）、不是明細子表的資料類別。端點、權限與錯誤的完整說明見
//! [`hammer_shared::form_layout`]。
//!
//! 寫入都在單一 transaction 內完成，並寫入 `audit_logs`（`entity_key` 為 [`FORM_LAYOUT_AUDIT_KEY`]，
//! `record_id` 為版面 id）。修改沒有任何變動時不寫入。
//!
//! `config` 的檢查與讀取時的處理在 [`validate_config`]、[`sanitize_config`]、[`generated_config`]
//! （純邏輯，有單元測試）。

use std::collections::HashSet;

use hammer_db::DbError;
use hammer_db::ensure_version_matched;
use hammer_db::form_layout::{self as db, ENTITY_UNIQUE, FormLayoutRow};
use hammer_db::schema as schema_db;
use hammer_db::sqlx::{PgConnection, PgPool, PgTransaction};
use hammer_shared::form_layout::{
    BLOCKS_SECTION_KEY, BlockPlacement, FormItem, FormLayout, FormLayoutConfig, FormSection,
    MAIN_SECTION_KEY, MAX_COLUMNS, MAX_ITEMS, MAX_SECTION_KEY_CHARS, MAX_SECTION_TITLE_CHARS,
    MAX_SECTIONS, OTHER_SECTION_KEY, OTHER_SECTION_TITLE, PRESET_COLUMNS, SaveFormLayoutRequest,
    preset_sections, system_blocks,
};
use hammer_shared::schema::{EntityDefinition, EntityKind, FieldDefinition};
use hammer_shared::{Id, audit as action, new_id, permission};
use serde_json::{Value, json};
use time::OffsetDateTime;

use crate::CoreError;
use crate::audit::{self, AuditDiff, AuditEntry};
use crate::auth::AuthContext;
use crate::schema::{ENTITY_NOT_FOUND, ensure_version, entity_from_row, field_from_row};

/// 表單版面在 `audit_logs.entity_key` 的值。
pub const FORM_LAYOUT_AUDIT_KEY: &str = "form_layout";
/// 還原成預設時，資料類別沒有設定版面的錯誤訊息。
pub const FORM_LAYOUT_NOT_FOUND: &str = "此資料類別沒有設定表單版面";
/// 版面已被其他人建立或還原（`version` 與目前狀態不符）時的錯誤訊息。
pub const FORM_LAYOUT_CHANGED: &str = "表單版面已被其他人修改，請重新載入";

/// 驗證錯誤 `details.field` 的路徑。
pub mod path {
    pub const SECTIONS: &str = "config.sections";

    /// `config.sections[i].<prop>`
    pub fn section(index: usize, prop: &str) -> String {
        format!("config.sections[{index}].{prop}")
    }

    /// `config.sections[i].items[j].<prop>`
    pub fn item(section: usize, item: usize, prop: &str) -> String {
        format!("config.sections[{section}].items[{item}].{prop}")
    }
}

/// 檢查與正規化需要的資料類別資訊。
#[derive(Debug, Clone, Copy)]
pub struct FormSchema<'a> {
    /// 資料類別代碼（決定適用的系統區塊）。
    pub entity_key: &'a str,
    /// 主表所有欄位定義（含已停用）。
    pub fields: &'a [FieldDefinition],
    /// 有效的明細子表代碼，依明細子表的 `sort_order`。
    pub detail_keys: &'a [&'a str],
}

impl FormSchema<'_> {
    fn blocks(&self) -> Vec<(String, BlockPlacement)> {
        system_blocks(self.entity_key, self.detail_keys)
    }

    fn is_active_field(&self, key: &str) -> bool {
        self.fields
            .iter()
            .any(|f| f.field_key == key && f.is_active)
    }

    /// 有效且顯示的欄位，依 `sort_order`（同值依 id）。
    fn visible_fields(&self) -> Vec<&FieldDefinition> {
        let mut visible: Vec<&FieldDefinition> = self
            .fields
            .iter()
            .filter(|f| f.is_active && f.is_visible)
            .collect();
        visible.sort_by_key(|f| (f.sort_order, f.id));
        visible
    }
}

// ---------------------------------------------------------------------------
// 查詢
// ---------------------------------------------------------------------------

/// 取得資料類別的表單版面（已正規化）；沒有設定時依欄位定義自動產生。需要 `record.read`。
pub async fn get_form_layout(
    pool: &PgPool,
    ctx: &AuthContext,
    entity_key: &str,
) -> Result<FormLayout, CoreError> {
    ctx.require(permission::RECORD_READ)?;
    let mut conn = pool.acquire().await.map_err(DbError::from)?;
    let loaded = load_entity(&mut conn, entity_key, false).await?;
    let row = db::find_by_entity(&mut *conn, loaded.entity.id).await?;
    Ok(loaded.layout(row))
}

// ---------------------------------------------------------------------------
// 寫入
// ---------------------------------------------------------------------------

/// 新增或修改表單版面。需要 `schema.manage`。
///
/// - `config` 不合法 → [`CoreError::InvalidParam`]（路徑見 [`path`]）
/// - 第一次建立時 `version` 為 `null`；已有版面時必須帶目前的 `version`，不符 → `Conflict`
/// - 沒有任何變動時不寫入，回傳目前的版面（`version` 不變）
pub async fn save_form_layout(
    pool: &PgPool,
    ctx: &AuthContext,
    entity_key: &str,
    req: &SaveFormLayoutRequest,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<FormLayout, CoreError> {
    ctx.require(permission::SCHEMA_MANAGE)?;

    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let loaded = load_entity(&mut tx, entity_key, true).await?;
    let config = loaded.with_schema(|schema| validate_config(schema, &req.config))?;
    let config_value = config_json(&config)?;
    let current = db::find_by_entity_for_update(&mut *tx, loaded.entity.id).await?;

    let id = match (current, req.version) {
        (None, None) => {
            let id = new_id();
            db::insert(
                &mut *tx,
                id,
                loaded.entity.id,
                &config_value,
                ctx.user_id,
                now,
            )
            .await
            .map_err(write_error)?;
            let after = snapshot(&loaded.entity.key, &config_value);
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
            id
        }
        (Some(row), Some(version)) => {
            ensure_version(row.version, version)?;
            if row.config == config_value {
                commit(tx).await?;
                return Ok(loaded.layout(Some(row)));
            }
            ensure_version_matched(
                db::update(&mut *tx, row.id, version, &config_value, ctx.user_id).await?,
            )?;
            record_audit(
                &mut tx,
                ctx,
                action::UPDATE,
                row.id,
                AuditDiff::between(
                    Some(&snapshot(&loaded.entity.key, &row.config)),
                    Some(&snapshot(&loaded.entity.key, &config_value)),
                )?,
                client_info,
                now,
            )
            .await?;
            row.id
        }
        // 讀取後被其他人建立或還原
        (None, Some(_)) | (Some(_), None) => {
            return Err(CoreError::Conflict(FORM_LAYOUT_CHANGED.into()));
        }
    };

    let row = db::find_by_entity(&mut *tx, loaded.entity.id).await?;
    commit(tx).await?;
    tracing::info!(actor = %ctx.user_id, entity = %loaded.entity.key, layout_id = %id, "已儲存表單版面");
    Ok(loaded.layout(row))
}

/// 還原成預設（軟刪除版面），回傳自動產生的版面。需要 `schema.manage`。
///
/// - 沒有設定版面 → [`CoreError::NotFound`]
/// - `version` 不符 → `Conflict`（§9.4）
pub async fn reset_form_layout(
    pool: &PgPool,
    ctx: &AuthContext,
    entity_key: &str,
    version: i32,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<FormLayout, CoreError> {
    ctx.require(permission::SCHEMA_MANAGE)?;

    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let loaded = load_entity(&mut tx, entity_key, true).await?;
    let current = db::find_by_entity_for_update(&mut *tx, loaded.entity.id)
        .await?
        .ok_or_else(|| CoreError::NotFound(FORM_LAYOUT_NOT_FOUND.into()))?;
    ensure_version(current.version, version)?;

    ensure_version_matched(
        db::soft_delete(&mut *tx, current.id, version, now, ctx.user_id).await?,
    )?;
    record_audit(
        &mut tx,
        ctx,
        action::DELETE,
        current.id,
        AuditDiff::between(Some(&snapshot(&loaded.entity.key, &current.config)), None)?,
        client_info,
        now,
    )
    .await?;
    commit(tx).await?;

    tracing::info!(actor = %ctx.user_id, entity = %loaded.entity.key, layout_id = %current.id, "已還原表單版面");
    Ok(loaded.layout(None))
}

// ---------------------------------------------------------------------------
// config（純邏輯）
// ---------------------------------------------------------------------------

/// 寫入前檢查 `config`，錯誤為 [`CoreError::InvalidParam`]（`param` 見 [`path`]）。
/// 回傳要儲存的設定：標題去除前後空白；保留代碼 [`OTHER_SECTION_KEY`]、[`BLOCKS_SECTION_KEY`]
/// 改名為 `other`、`blocks`（已有同名的區塊時依序加上 `_2`、`_3`）。
///
/// 規則見 docs/architecture.md §7.7「config 結構」「寫入時的檢查」。
pub fn validate_config(
    schema: &FormSchema<'_>,
    config: &FormLayoutConfig,
) -> Result<FormLayoutConfig, CoreError> {
    let invalid = |param: String, message: &str| CoreError::InvalidParam {
        param,
        message: message.to_owned(),
    };
    if config.sections.is_empty() {
        return Err(invalid(path::SECTIONS.into(), "表單版面至少要有一個區塊"));
    }
    if config.sections.len() > MAX_SECTIONS {
        return Err(invalid(
            path::SECTIONS.into(),
            &format!("表單版面最多 {MAX_SECTIONS} 個區塊"),
        ));
    }
    let total: usize = config.sections.iter().map(|s| s.items.len()).sum();
    if total > MAX_ITEMS {
        return Err(invalid(
            path::SECTIONS.into(),
            &format!("表單版面的項目合計最多 {MAX_ITEMS} 個"),
        ));
    }

    let keys = rename_reserved_keys(&config.sections);
    let blocks: HashSet<String> = schema.blocks().into_iter().map(|(code, _)| code).collect();
    let mut seen_keys = HashSet::new();
    let mut seen_fields = HashSet::new();
    let mut seen_blocks = HashSet::new();
    let mut sections = Vec::with_capacity(config.sections.len());

    for (i, (section, key)) in config.sections.iter().zip(keys).enumerate() {
        if !is_valid_section_key(&key) {
            return Err(invalid(
                path::section(i, "key"),
                "區塊代碼只能使用小寫英文字母、數字與底線，且必須以英文字母開頭",
            ));
        }
        if !seen_keys.insert(key.clone()) {
            return Err(invalid(path::section(i, "key"), "區塊代碼重複"));
        }
        let title = section.title.trim();
        if title.chars().count() > MAX_SECTION_TITLE_CHARS {
            return Err(invalid(
                path::section(i, "title"),
                &format!("區塊標題最多 {MAX_SECTION_TITLE_CHARS} 個字元"),
            ));
        }
        if title.chars().any(char::is_control) {
            return Err(invalid(path::section(i, "title"), "區塊標題不可有控制字元"));
        }
        if !(1..=MAX_COLUMNS).contains(&section.columns) {
            return Err(invalid(
                path::section(i, "columns"),
                &format!("欄數必須是 1 到 {MAX_COLUMNS}"),
            ));
        }
        if section.collapsible && title.is_empty() {
            return Err(invalid(
                path::section(i, "collapsible"),
                "可以收折的區塊必須有標題",
            ));
        }
        if section.collapsed && !section.collapsible {
            return Err(invalid(
                path::section(i, "collapsed"),
                "只有可以收折的區塊能預設收折",
            ));
        }

        let span_ok = |span: i32| (1..=section.columns).contains(&span);
        let span_message = format!("寬度必須是 1 到 {} 欄", section.columns);
        for (j, item) in section.items.iter().enumerate() {
            match item {
                FormItem::Field { field, span } => {
                    if !schema.is_active_field(field) {
                        return Err(invalid(
                            path::item(i, j, "field"),
                            &format!("沒有可使用的欄位「{field}」"),
                        ));
                    }
                    if !seen_fields.insert(field.as_str()) {
                        return Err(invalid(
                            path::item(i, j, "field"),
                            &format!("欄位「{field}」重複"),
                        ));
                    }
                    if !span_ok(*span) {
                        return Err(invalid(path::item(i, j, "span"), &span_message));
                    }
                }
                FormItem::Spacer { span } => {
                    if !span_ok(*span) {
                        return Err(invalid(path::item(i, j, "span"), &span_message));
                    }
                }
                FormItem::Block { block } => {
                    if !blocks.contains(block) {
                        return Err(invalid(
                            path::item(i, j, "block"),
                            &format!("此資料類別沒有系統區塊「{block}」"),
                        ));
                    }
                    if !seen_blocks.insert(block.as_str()) {
                        return Err(invalid(
                            path::item(i, j, "block"),
                            &format!("系統區塊「{block}」重複"),
                        ));
                    }
                }
            }
        }

        sections.push(FormSection {
            key,
            title: title.to_owned(),
            columns: section.columns,
            collapsible: section.collapsible,
            collapsed: section.collapsed,
            items: section.items.clone(),
        });
    }
    Ok(FormLayoutConfig { sections })
}

/// 讀取時的處理，不回傳錯誤（docs/architecture.md §7.7「讀取時的處理」）：
///
/// 1. 略過已停用或不存在的欄位、不再適用的系統區塊，以及重複的欄位與系統區塊；
///    欄數限制在 1–[`MAX_COLUMNS`]，`span` 超過所在區塊的欄數時調整為欄數
/// 2. 有效、顯示、但沒有放進版面的欄位，依 `sort_order` 附加到 [`OTHER_SECTION_KEY`] 區塊
/// 3. 沒有放進版面的系統區塊：[`BlockPlacement::Top`] 插入第一個區塊的最前面，
///    [`BlockPlacement::Bottom`] 附加到最後的 [`BLOCKS_SECTION_KEY`] 區塊
///
/// 略過後沒有項目的區塊仍然保留。
pub fn sanitize_config(schema: &FormSchema<'_>, config: FormLayoutConfig) -> FormLayoutConfig {
    let blocks = schema.blocks();
    let mut seen_fields: HashSet<String> = HashSet::new();
    let mut seen_blocks: HashSet<String> = HashSet::new();

    let mut sections: Vec<FormSection> = config
        .sections
        .into_iter()
        .map(|section| {
            let columns = section.columns.clamp(1, MAX_COLUMNS);
            let items = section
                .items
                .into_iter()
                .filter_map(|item| match item {
                    FormItem::Field { field, span } => (schema.is_active_field(&field)
                        && seen_fields.insert(field.clone()))
                    .then(|| FormItem::Field {
                        field,
                        span: span.clamp(1, columns),
                    }),
                    FormItem::Spacer { span } => Some(FormItem::Spacer {
                        span: span.clamp(1, columns),
                    }),
                    FormItem::Block { block } => (blocks.iter().any(|(code, _)| *code == block)
                        && seen_blocks.insert(block.clone()))
                    .then_some(FormItem::Block { block }),
                })
                .collect();
            FormSection {
                columns,
                collapsed: section.collapsed && section.collapsible,
                items,
                ..section
            }
        })
        .collect();

    let missing_fields: Vec<FormItem> = schema
        .visible_fields()
        .into_iter()
        .filter(|f| !seen_fields.contains(&f.field_key))
        .map(|f| field_item(&f.field_key))
        .collect();
    let (top, bottom): (Vec<_>, Vec<_>) = blocks
        .into_iter()
        .filter(|(code, _)| !seen_blocks.contains(code))
        .partition(|(_, placement)| *placement == BlockPlacement::Top);

    if !top.is_empty() {
        if sections.is_empty() {
            sections.push(plain_section(MAIN_SECTION_KEY, "", Vec::new()));
        }
        let first = &mut sections[0].items;
        first.splice(0..0, top.into_iter().map(|(code, _)| block_item(code)));
    }
    if !missing_fields.is_empty() {
        sections.push(plain_section(
            OTHER_SECTION_KEY,
            OTHER_SECTION_TITLE,
            missing_fields,
        ));
    }
    if !bottom.is_empty() {
        sections.push(plain_section(
            BLOCKS_SECTION_KEY,
            "",
            bottom
                .into_iter()
                .map(|(code, _)| block_item(code))
                .collect(),
        ));
    }
    FormLayoutConfig { sections }
}

/// 沒有設定版面時依欄位定義產生：
///
/// - 有分組預設版面（[`preset_sections`]，客戶、廠商）：預設的區塊（2 欄），略過停用、不顯示的
///   欄位與沒有欄位的區塊，再依 [`sanitize_config`] 附加單號、未列出的欄位（含自訂欄位）與
///   明細表
/// - 其他資料類別（與 M8 之前的畫面相同）：一個單欄、沒有標題的 [`MAIN_SECTION_KEY`] 區塊，
///   依序為欄位之前的系統區塊、有效且顯示的欄位（依 `sort_order`）、欄位之後的系統區塊
pub fn generated_config(schema: &FormSchema<'_>) -> FormLayoutConfig {
    if let Some(preset) = preset_sections(schema.entity_key) {
        let visible: HashSet<&str> = schema
            .visible_fields()
            .into_iter()
            .map(|f| f.field_key.as_str())
            .collect();
        let sections = preset
            .into_iter()
            .filter_map(|p| {
                let items: Vec<FormItem> = p
                    .fields
                    .iter()
                    .filter(|(key, _)| visible.contains(key))
                    .map(|&(key, span)| FormItem::Field {
                        field: key.to_owned(),
                        span,
                    })
                    .collect();
                (!items.is_empty()).then(|| FormSection {
                    key: p.key.to_owned(),
                    title: p.title.to_owned(),
                    columns: PRESET_COLUMNS,
                    collapsible: false,
                    collapsed: false,
                    items,
                })
            })
            .collect();
        return sanitize_config(schema, FormLayoutConfig { sections });
    }
    let (top, bottom): (Vec<_>, Vec<_>) = schema
        .blocks()
        .into_iter()
        .partition(|(_, placement)| *placement == BlockPlacement::Top);
    let items = top
        .into_iter()
        .map(|(code, _)| block_item(code))
        .chain(
            schema
                .visible_fields()
                .into_iter()
                .map(|f| field_item(&f.field_key)),
        )
        .chain(bottom.into_iter().map(|(code, _)| block_item(code)))
        .collect();
    FormLayoutConfig {
        sections: vec![plain_section(MAIN_SECTION_KEY, "", items)],
    }
}

fn field_item(key: &str) -> FormItem {
    FormItem::Field {
        field: key.to_owned(),
        span: 1,
    }
}

fn block_item(code: String) -> FormItem {
    FormItem::Block { block: code }
}

fn plain_section(key: &str, title: &str, items: Vec<FormItem>) -> FormSection {
    FormSection {
        key: key.to_owned(),
        title: title.to_owned(),
        columns: 1,
        collapsible: false,
        collapsed: false,
        items,
    }
}

/// `^[a-z][a-z0-9_]{0,49}$`
fn is_valid_section_key(key: &str) -> bool {
    let mut chars = key.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && key.chars().count() <= MAX_SECTION_KEY_CHARS
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// 各區塊要儲存的代碼：保留代碼改名為去掉 `_` 的名稱，與其他區塊同名時依序加上 `_2`、`_3`。
fn rename_reserved_keys(sections: &[FormSection]) -> Vec<String> {
    let is_reserved = |key: &str| key == OTHER_SECTION_KEY || key == BLOCKS_SECTION_KEY;
    let mut taken: HashSet<String> = sections
        .iter()
        .filter(|s| !is_reserved(&s.key))
        .map(|s| s.key.clone())
        .collect();
    sections
        .iter()
        .map(|s| {
            if !is_reserved(&s.key) {
                return s.key.clone();
            }
            let base = s.key.trim_start_matches('_');
            let key = std::iter::once(base.to_owned())
                .chain((2..).map(|n| format!("{base}_{n}")))
                .find(|candidate| !taken.contains(candidate))
                .unwrap_or_else(|| base.to_owned());
            taken.insert(key.clone());
            key
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 共用
// ---------------------------------------------------------------------------

/// 有效的資料類別、主表欄位定義（含已停用）與有效的明細子表。
struct LoadedEntity {
    entity: EntityDefinition,
    fields: Vec<FieldDefinition>,
    detail_keys: Vec<String>,
}

impl LoadedEntity {
    fn with_schema<T>(&self, f: impl FnOnce(&FormSchema<'_>) -> T) -> T {
        let keys: Vec<&str> = self.detail_keys.iter().map(String::as_str).collect();
        f(&FormSchema {
            entity_key: &self.entity.key,
            fields: &self.fields,
            detail_keys: &keys,
        })
    }

    /// 資料列轉成 API 回應（正規化 `config`）；沒有版面時自動產生。
    /// `config` 無法解析時（不應發生）視為沒有設定版面，只寫進 log。
    fn layout(&self, row: Option<FormLayoutRow>) -> FormLayout {
        let Some(row) = row else {
            return FormLayout {
                id: None,
                version: None,
                is_default: true,
                config: self.with_schema(generated_config),
            };
        };
        let config = match serde_json::from_value::<FormLayoutConfig>(row.config) {
            Ok(config) => self.with_schema(|schema| sanitize_config(schema, config)),
            Err(err) => {
                tracing::warn!(layout_id = %row.id, error = %err, "form_layouts.config 格式錯誤，視為沒有設定版面");
                self.with_schema(generated_config)
            }
        };
        FormLayout {
            id: Some(row.id),
            version: Some(row.version),
            is_default: false,
            config,
        }
    }
}

/// 載入有效、不是明細子表的資料類別。`for_write` 時以 `FOR SHARE` 鎖定資料類別與明細子表，
/// 等待進行中的定義變更完成（必須在 transaction 內）。
async fn load_entity(
    conn: &mut PgConnection,
    key: &str,
    for_write: bool,
) -> Result<LoadedEntity, CoreError> {
    let row = if for_write {
        schema_db::find_entity_by_key_for_share(&mut *conn, key).await?
    } else {
        schema_db::find_entity_by_key(&mut *conn, key).await?
    };
    let entity = row
        .map(entity_from_row)
        .transpose()?
        .filter(|e| e.is_active && e.kind != EntityKind::Detail)
        .ok_or_else(|| CoreError::NotFound(ENTITY_NOT_FOUND.into()))?;
    let fields = schema_db::list_fields(&mut *conn, entity.id)
        .await?
        .into_iter()
        .map(field_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    let details = if for_write {
        schema_db::list_detail_entities_for_share(&mut *conn, entity.id).await?
    } else {
        schema_db::list_detail_entities(&mut *conn, entity.id).await?
    };
    let detail_keys = details
        .into_iter()
        .filter(|d| d.is_active)
        .map(|d| d.key)
        .collect();
    Ok(LoadedEntity {
        entity,
        fields,
        detail_keys,
    })
}

fn config_json(config: &FormLayoutConfig) -> Result<Value, CoreError> {
    serde_json::to_value(config)
        .map_err(|e| CoreError::Internal(format!("序列化表單版面失敗：{e}")))
}

/// 操作紀錄的快照。
fn snapshot(entity_key: &str, config: &Value) -> Value {
    json!({ "entity_key": entity_key, "config": config })
}

/// 同時有其他請求建立了版面（違反唯一索引）→ `Conflict`。
fn write_error(err: DbError) -> CoreError {
    match err.unique_violation() {
        Some(ENTITY_UNIQUE) => CoreError::Conflict(FORM_LAYOUT_CHANGED.into()),
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
            entity_key: Some(FORM_LAYOUT_AUDIT_KEY),
            record_id: Some(layout_id),
            diff,
            client_info,
            occurred_at: now,
        },
    )
    .await?;
    Ok(())
}

async fn commit(tx: PgTransaction<'_>) -> Result<(), CoreError> {
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hammer_shared::schema::{DataType, FieldStorage};

    fn field(key: &str, sort_order: i32) -> FieldDefinition {
        FieldDefinition {
            id: new_id(),
            entity_id: Id::nil(),
            field_key: key.to_owned(),
            label: key.to_owned(),
            data_type: DataType::Text,
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
            sort_order,
            is_active: true,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            version: 1,
        }
    }

    /// `a`(1)、`b`(2)、`c`(3)、`hidden`(4，不顯示)、`old`(0，已停用)。
    fn fields() -> Vec<FieldDefinition> {
        let mut hidden = field("hidden", 4);
        hidden.is_visible = false;
        let mut old = field("old", 0);
        old.is_active = false;
        vec![field("c", 3), field("a", 1), field("b", 2), hidden, old]
    }

    const DETAILS: &[&str] = &["parts"];

    fn schema<'a>(entity_key: &'a str, fields: &'a [FieldDefinition]) -> FormSchema<'a> {
        FormSchema {
            entity_key,
            fields,
            detail_keys: DETAILS,
        }
    }

    fn f(key: &str, span: i32) -> FormItem {
        FormItem::Field {
            field: key.into(),
            span,
        }
    }

    fn b(code: &str) -> FormItem {
        FormItem::Block { block: code.into() }
    }

    fn section(key: &str, columns: i32, items: Vec<FormItem>) -> FormSection {
        FormSection {
            key: key.into(),
            title: String::new(),
            columns,
            collapsible: false,
            collapsed: false,
            items,
        }
    }

    fn config(sections: Vec<FormSection>) -> FormLayoutConfig {
        FormLayoutConfig { sections }
    }

    fn param_of(result: Result<FormLayoutConfig, CoreError>) -> String {
        match result {
            Err(CoreError::InvalidParam { param, message }) => {
                assert!(!message.is_empty());
                param
            }
            other => panic!("預期 InvalidParam，實際為 {other:?}"),
        }
    }

    #[test]
    fn valid_config_passes_and_trims_titles() {
        let fields = fields();
        let mut basic = section(
            "basic",
            2,
            vec![
                b("record_no"),
                f("a", 1),
                FormItem::Spacer { span: 1 },
                f("b", 2),
                f("hidden", 1),
            ],
        );
        basic.title = "  基本資料 ".into();
        basic.collapsible = true;
        basic.collapsed = true;
        let input = config(vec![basic, section("lines", 1, vec![b("lines:parts")])]);
        let output = validate_config(&schema("repair", &fields), &input).unwrap();
        assert_eq!(output.sections[0].title, "基本資料");
        assert_eq!(output.sections[0].items, input.sections[0].items);
        assert_eq!(output.sections[1], input.sections[1]);
    }

    #[test]
    fn invalid_structure_is_rejected() {
        let fields = fields();
        let s = schema("repair", &fields);
        let check = |sections: Vec<FormSection>| param_of(validate_config(&s, &config(sections)));

        assert_eq!(check(vec![]), "config.sections");
        assert_eq!(
            check(
                (0..=MAX_SECTIONS)
                    .map(|i| section(&format!("s{i}"), 1, vec![]))
                    .collect()
            ),
            "config.sections"
        );
        let spacers = vec![FormItem::Spacer { span: 1 }; MAX_ITEMS + 1];
        assert_eq!(check(vec![section("a", 1, spacers)]), "config.sections");

        for key in ["", "A", "1a", "a-b", "_x", &"a".repeat(51)] {
            assert_eq!(
                check(vec![section(key, 1, vec![])]),
                "config.sections[0].key",
                "{key}"
            );
        }
        assert_eq!(
            check(vec![section("a", 1, vec![]), section("a", 1, vec![])]),
            "config.sections[1].key"
        );

        let mut long = section("a", 1, vec![]);
        long.title = "字".repeat(51);
        assert_eq!(check(vec![long]), "config.sections[0].title");
        let mut control = section("a", 1, vec![]);
        control.title = "a\nb".into();
        assert_eq!(check(vec![control]), "config.sections[0].title");

        for columns in [0, 5, -1] {
            assert_eq!(
                check(vec![section("a", columns, vec![])]),
                "config.sections[0].columns"
            );
        }

        let mut collapsible = section("a", 1, vec![]);
        collapsible.collapsible = true;
        assert_eq!(check(vec![collapsible]), "config.sections[0].collapsible");
        let mut collapsed = section("a", 1, vec![]);
        collapsed.title = "x".into();
        collapsed.collapsed = true;
        assert_eq!(check(vec![collapsed]), "config.sections[0].collapsed");
    }

    #[test]
    fn invalid_items_are_rejected() {
        let fields = fields();
        let s = schema("repair", &fields);
        let check = |items: Vec<FormItem>| {
            param_of(validate_config(&s, &config(vec![section("a", 2, items)])))
        };

        assert_eq!(
            check(vec![f("old", 1)]),
            "config.sections[0].items[0].field"
        );
        assert_eq!(
            check(vec![f("missing", 1)]),
            "config.sections[0].items[0].field"
        );
        assert_eq!(
            check(vec![f("a", 1), f("a", 1)]),
            "config.sections[0].items[1].field"
        );
        assert_eq!(check(vec![f("a", 3)]), "config.sections[0].items[0].span");
        assert_eq!(check(vec![f("a", 0)]), "config.sections[0].items[0].span");
        assert_eq!(
            check(vec![FormItem::Spacer { span: 3 }]),
            "config.sections[0].items[0].span"
        );
        assert_eq!(
            check(vec![b("lines:other")]),
            "config.sections[0].items[0].block"
        );
        // 銷項發票才有「選擇出貨單」
        assert_eq!(
            check(vec![b("shipment_picker")]),
            "config.sections[0].items[0].block"
        );
        assert_eq!(
            check(vec![b("record_no"), b("record_no")]),
            "config.sections[0].items[1].block"
        );

        // 同一個欄位跨區塊重複也不行
        let result = validate_config(
            &s,
            &config(vec![
                section("x", 1, vec![f("a", 1)]),
                section("y", 1, vec![f("a", 1)]),
            ]),
        );
        assert_eq!(param_of(result), "config.sections[1].items[0].field");
    }

    #[test]
    fn system_blocks_depend_on_entity() {
        let fields = fields();
        let ok = |key: &str, block: &str| {
            validate_config(
                &schema(key, &fields),
                &config(vec![section("a", 1, vec![b(block)])]),
            )
            .is_ok()
        };
        assert!(ok("sales_invoice", "shipment_picker"));
        assert!(ok("purchase_invoice", "receipt_picker"));
        assert!(ok("sales_allowance", "allowance_available"));
        assert!(ok("shipment", "used_lots"));
        assert!(!ok("purchase_invoice", "shipment_picker"));
        assert!(!ok("goods_receipt", "used_lots"));
    }

    #[test]
    fn reserved_keys_are_renamed_on_write() {
        let fields = fields();
        let input = config(vec![
            section("other", 1, vec![]),
            section(OTHER_SECTION_KEY, 1, vec![f("a", 1)]),
            section(BLOCKS_SECTION_KEY, 1, vec![b("lines:parts")]),
        ]);
        let output = validate_config(&schema("repair", &fields), &input).unwrap();
        let keys: Vec<&str> = output.sections.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(keys, ["other", "other_2", "blocks"]);
    }

    #[test]
    fn sanitize_drops_invalid_references_without_error() {
        let fields = fields();
        let s = schema("repair", &fields);
        let stored = config(vec![section(
            "a",
            2,
            vec![
                b("record_no"),
                f("old", 1),
                f("missing", 1),
                f("a", 3),
                f("a", 1),
                b("lines:removed"),
                b("used_lots"),
                f("hidden", 1),
                f("b", 1),
                f("c", 2),
                b("lines:parts"),
            ],
        )]);
        let output = sanitize_config(&s, stored);
        assert_eq!(
            output,
            config(vec![section(
                "a",
                2,
                vec![
                    b("record_no"),
                    f("a", 2),
                    f("hidden", 1),
                    f("b", 1),
                    f("c", 2),
                    b("lines:parts"),
                ],
            )])
        );
    }

    #[test]
    fn sanitize_appends_missing_fields_and_blocks() {
        let fields = fields();
        let s = schema("sales_allowance", &fields);
        let stored = config(vec![
            section("first", 2, vec![f("b", 1)]),
            section("empty", 1, vec![f("old", 1)]),
        ]);
        let output = sanitize_config(&s, stored);
        let mut other = section(OTHER_SECTION_KEY, 1, vec![f("a", 1), f("c", 1)]);
        other.title = OTHER_SECTION_TITLE.into();
        assert_eq!(
            output,
            config(vec![
                section(
                    "first",
                    2,
                    vec![b("allowance_available"), b("record_no"), f("b", 1)],
                ),
                // 略過後沒有項目的區塊仍然保留
                section("empty", 1, vec![]),
                other,
                section(BLOCKS_SECTION_KEY, 1, vec![b("lines:parts")]),
            ])
        );
    }

    #[test]
    fn sanitize_keeps_complete_config_unchanged() {
        let fields = fields();
        let s = schema("repair", &fields);
        let mut collapsible = section("x", 3, vec![f("c", 3), b("lines:parts")]);
        collapsible.title = "其他".into();
        collapsible.collapsible = true;
        collapsible.collapsed = true;
        let stored = config(vec![
            section("main", 2, vec![b("record_no"), f("a", 1), f("b", 1)]),
            collapsible,
        ]);
        assert_eq!(sanitize_config(&s, stored.clone()), stored);
    }

    #[test]
    fn generated_config_matches_current_form() {
        let fields = fields();
        assert_eq!(
            generated_config(&schema("repair", &fields)),
            config(vec![section(
                MAIN_SECTION_KEY,
                1,
                vec![
                    b("record_no"),
                    f("a", 1),
                    f("b", 1),
                    f("c", 1),
                    b("lines:parts"),
                ],
            )])
        );
        let generated = generated_config(&FormSchema {
            entity_key: "sales_invoice",
            fields: &fields,
            detail_keys: &["sales_invoice_line"],
        });
        let items = &generated.sections[0].items;
        assert_eq!(items[0], b("record_no"));
        assert_eq!(
            items[4..],
            [b("shipment_picker"), b("lines:sales_invoice_line")]
        );
        // 產生的版面寫回時也合法
        assert!(
            validate_config(
                &schema("repair", &fields),
                &generated_config(&schema("repair", &fields))
            )
            .is_ok()
        );
    }

    /// 與桌面程式（`records/formLayout.ts` 的 `generatedFormLayout`）共用的測試資料：
    /// 客戶、廠商的分組預設版面必須等於 `expected`（#254）。
    #[test]
    fn shared_preset_cases_match_expected() {
        let doc: Value = serde_json::from_str(include_str!(
            "../../shared/testdata/generated_form_layouts.json"
        ))
        .unwrap();
        for case in doc["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let fields: Vec<FieldDefinition> = case["fields"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| {
                    let mut def = field(
                        v["field_key"].as_str().unwrap(),
                        i32::try_from(v["sort_order"].as_i64().unwrap()).unwrap(),
                    );
                    def.is_active = v["is_active"].as_bool().unwrap();
                    def.is_visible = v["is_visible"].as_bool().unwrap();
                    def
                })
                .collect();
            let detail_keys: Vec<&str> = case["detail_keys"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect();
            let schema = FormSchema {
                entity_key: case["entity_key"].as_str().unwrap(),
                fields: &fields,
                detail_keys: &detail_keys,
            };
            let expected: FormLayoutConfig =
                serde_json::from_value(case["expected"].clone()).unwrap();
            let generated = generated_config(&schema);
            assert_eq!(generated, expected, "{name}");
            // 產生的版面寫回時也合法，讀取時不再變動
            assert!(validate_config(&schema, &generated).is_ok(), "{name}");
            assert_eq!(
                sanitize_config(&schema, generated.clone()),
                generated,
                "{name}"
            );
        }
    }

    #[test]
    fn other_entities_keep_single_column_generated_config() {
        assert!(preset_sections("product").is_none());
        let fields = fields();
        let generated = generated_config(&schema("product", &fields));
        assert_eq!(generated.sections.len(), 1);
        assert_eq!(generated.sections[0].key, MAIN_SECTION_KEY);
        assert_eq!(generated.sections[0].columns, 1);
    }

    #[test]
    fn non_unique_db_errors_pass_through() {
        assert!(matches!(
            write_error(DbError::Conflict),
            CoreError::Db(DbError::Conflict)
        ));
    }
}

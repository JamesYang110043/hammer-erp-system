//! 欄位的動態索引規劃（docs/architecture.md §7.5，純邏輯，不存取資料庫）。
//!
//! - [`index_name`]：由資料類別代碼與欄位代碼推算索引名稱
//! - [`plan_index_changes`]：比較欄位定義修改前後，決定要建立或移除哪些索引
//!
//! 索引是否存在只由欄位定義決定：**啟用中**且 `is_searchable` → 可搜尋索引；
//! **啟用中**且 `is_unique` → 部分唯一索引。停用欄位時移除，重新啟用時再建立。
//! 執行 DDL 在 [`hammer_db::dynamic_index`]。
//!
//! # 索引名稱
//!
//! `{前綴}_{entity_key}_{field_key}_{雜湊}`：
//!
//! - 前綴：可搜尋 `cfs`、不可重複 `cfu`
//! - 前綴與兩個代碼的部分超過 [`READABLE_MAX_LEN`] 個字元時截斷
//! - 雜湊：`entity_key` 與 `field_key` 的 SHA-256 前 8 個十六進位字元。
//!   代碼可含底線，`{entity}_{field}` 本身可能有歧義（`a_b` + `c` 與 `a` + `b_c`），
//!   截斷後也可能相同，雜湊用來區分這些情況
//!
//! 名稱最長 [`READABLE_MAX_LEN`] + 9 = 63 個字元，符合 PostgreSQL 識別字的上限。
//! 資料類別代碼與欄位代碼（含已停用）建立後都不可修改、不可重複使用，所以名稱固定不變。

use hammer_db::dynamic_index::MAX_IDENTIFIER_LEN;
use hammer_shared::Id;
use hammer_shared::schema::{FieldDefinition, validate_key};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::CoreError;

/// 名稱中雜湊的長度（十六進位字元）。
const HASH_LEN: usize = 8;
/// 名稱中可讀部分（前綴與兩個代碼）的長度上限。
pub const READABLE_MAX_LEN: usize = MAX_IDENTIFIER_LEN - 1 - HASH_LEN;

/// 動態索引的種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IndexKind {
    /// 可搜尋：運算式索引。
    Searchable,
    /// 不可重複：部分唯一索引。
    Unique,
}

impl IndexKind {
    fn prefix(self) -> &'static str {
        match self {
            Self::Searchable => "cfs",
            Self::Unique => "cfu",
        }
    }

    /// 欄位定義是否需要這種索引。
    pub fn wanted_by(self, def: &FieldDefinition) -> bool {
        def.is_active
            && match self {
                Self::Searchable => def.is_searchable,
                Self::Unique => def.is_unique,
            }
    }
}

/// 索引的變更。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexChange {
    Create(IndexKind),
    Drop(IndexKind),
}

/// 無法建立唯一索引時，重複的值（[`CoreError::DuplicateValues`]）。
#[derive(Debug, Clone, PartialEq)]
pub struct DuplicateValue {
    /// 重複的值（JSON 存值）。
    pub value: Value,
    /// 擁有這個值的未刪除資料筆數。
    pub count: i64,
    /// 擁有這個值的資料 id，依 id 排序，最多
    /// [`hammer_shared::schema::MAX_DUPLICATE_VALUES`] 筆。
    pub record_ids: Vec<Id>,
}

/// 推算索引名稱。代碼不符合命名規則時回傳 [`CoreError::Internal`]
/// （資料庫中的代碼都經過檢查，不應發生）。
pub fn index_name(kind: IndexKind, entity_key: &str, field_key: &str) -> Result<String, CoreError> {
    for key in [entity_key, field_key] {
        validate_key(key)
            .map_err(|e| CoreError::Internal(format!("代碼「{key}」不符合命名規則：{e}")))?;
    }
    let digest = Sha256::new()
        .chain_update(entity_key.as_bytes())
        .chain_update([0u8])
        .chain_update(field_key.as_bytes())
        .finalize();
    let hash: String = digest
        .iter()
        .take(HASH_LEN / 2)
        .map(|b| format!("{b:02x}"))
        .collect();
    let mut readable = format!("{}_{entity_key}_{field_key}", kind.prefix());
    // 只含 ASCII，可以直接以 byte 截斷
    readable.truncate(READABLE_MAX_LEN);
    Ok(format!("{readable}_{hash}"))
}

/// 欄位定義由 `current`（新增欄位時為 `None`）改為 `next` 時，需要的索引變更。
///
/// 移除排在建立之前。不需要變更時回傳空的清單。
pub fn plan_index_changes(
    current: Option<&FieldDefinition>,
    next: &FieldDefinition,
) -> Vec<IndexChange> {
    let mut drops = Vec::new();
    let mut creates = Vec::new();
    for kind in [IndexKind::Searchable, IndexKind::Unique] {
        let had = current.is_some_and(|c| kind.wanted_by(c));
        match (had, kind.wanted_by(next)) {
            (true, false) => drops.push(IndexChange::Drop(kind)),
            (false, true) => creates.push(IndexChange::Create(kind)),
            _ => {}
        }
    }
    drops.extend(creates);
    drops
}

#[cfg(test)]
mod tests {
    use super::*;
    use hammer_db::dynamic_index::is_safe_identifier;
    use hammer_shared::new_id;
    use hammer_shared::schema::{DataType, FieldStorage};
    use time::OffsetDateTime;

    fn def(is_active: bool, is_searchable: bool, is_unique: bool) -> FieldDefinition {
        FieldDefinition {
            id: new_id(),
            entity_id: Id::nil(),
            field_key: "code".into(),
            label: "代碼".into(),
            data_type: DataType::Text,
            is_system: false,
            storage: FieldStorage::Json,
            is_required: false,
            is_visible: true,
            is_readonly: false,
            is_searchable,
            is_unique,
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
            is_active,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            version: 1,
        }
    }

    #[test]
    fn name_is_readable_and_deterministic() {
        let name = index_name(IndexKind::Searchable, "repair_order", "title").unwrap();
        assert!(name.starts_with("cfs_repair_order_title_"), "{name}");
        assert_eq!(name.len(), "cfs_repair_order_title_".len() + HASH_LEN);
        assert!(is_safe_identifier(&name));
        assert_eq!(
            name,
            index_name(IndexKind::Searchable, "repair_order", "title").unwrap()
        );
        let unique = index_name(IndexKind::Unique, "repair_order", "title").unwrap();
        assert!(unique.starts_with("cfu_repair_order_title_"), "{unique}");
        // 雜湊只取決於代碼，與種類無關
        assert_eq!(
            name[name.len() - HASH_LEN..],
            unique[unique.len() - HASH_LEN..]
        );
    }

    #[test]
    fn long_keys_fit_postgres_identifier_limit() {
        let entity = "e".repeat(30);
        let field = "f".repeat(30);
        let name = index_name(IndexKind::Unique, &entity, &field).unwrap();
        assert_eq!(name.len(), MAX_IDENTIFIER_LEN);
        assert!(is_safe_identifier(&name));

        // 截斷後可讀部分相同，雜湊不同
        let other =
            index_name(IndexKind::Unique, &entity, &format!("{}g", "f".repeat(29))).unwrap();
        assert_eq!(other.len(), MAX_IDENTIFIER_LEN);
        assert_eq!(name[..READABLE_MAX_LEN], other[..READABLE_MAX_LEN]);
        assert_ne!(name, other);
    }

    #[test]
    fn ambiguous_underscores_get_different_names() {
        let a = index_name(IndexKind::Searchable, "a_b", "c").unwrap();
        let b = index_name(IndexKind::Searchable, "a", "b_c").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn invalid_keys_are_internal_errors() {
        for (entity, field) in [("repair_order", "x');drop"), ("Bad", "title"), ("a", "")] {
            assert!(matches!(
                index_name(IndexKind::Unique, entity, field),
                Err(CoreError::Internal(_))
            ));
        }
    }

    #[test]
    fn new_field_creates_wanted_indexes() {
        assert_eq!(plan_index_changes(None, &def(true, false, false)), vec![]);
        assert_eq!(
            plan_index_changes(None, &def(true, true, true)),
            vec![
                IndexChange::Create(IndexKind::Searchable),
                IndexChange::Create(IndexKind::Unique),
            ]
        );
    }

    #[test]
    fn toggling_flags() {
        let off = def(true, false, false);
        let searchable = def(true, true, false);
        let unique = def(true, false, true);
        assert_eq!(
            plan_index_changes(Some(&off), &searchable),
            vec![IndexChange::Create(IndexKind::Searchable)]
        );
        assert_eq!(
            plan_index_changes(Some(&searchable), &off),
            vec![IndexChange::Drop(IndexKind::Searchable)]
        );
        // 移除排在建立之前
        assert_eq!(
            plan_index_changes(Some(&searchable), &unique),
            vec![
                IndexChange::Drop(IndexKind::Searchable),
                IndexChange::Create(IndexKind::Unique),
            ]
        );
        assert_eq!(plan_index_changes(Some(&unique), &unique), vec![]);
    }

    #[test]
    fn deactivation_drops_and_reactivation_recreates() {
        let active = def(true, true, true);
        let inactive = def(false, true, true);
        assert_eq!(
            plan_index_changes(Some(&active), &inactive),
            vec![
                IndexChange::Drop(IndexKind::Searchable),
                IndexChange::Drop(IndexKind::Unique),
            ]
        );
        assert_eq!(
            plan_index_changes(Some(&inactive), &active),
            vec![
                IndexChange::Create(IndexKind::Searchable),
                IndexChange::Create(IndexKind::Unique),
            ]
        );
        // 停用中修改設定不建立索引
        assert_eq!(
            plan_index_changes(Some(&def(false, false, false)), &inactive),
            vec![]
        );
    }
}

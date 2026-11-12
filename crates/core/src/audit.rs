//! 操作紀錄的寫入（docs/architecture.md §8.4 `audit_logs`、docs/issue-workflow.md §5）。
//!
//! - [`AuditDiff::between`]：比對變更前後的 JSON 物件，只保留有變動的欄位
//! - [`client_info`]：由 `User-Agent` 與 `X-Client-Version` 組成 `client_info`
//! - [`record`]：在呼叫端的 transaction 內寫入一筆紀錄，與業務資料一起 commit 或 rollback
//!
//! 敏感欄位（[`SENSITIVE_FIELDS`]，例如 `password_hash`）一律不寫入紀錄。

use hammer_db::audit::{NewAuditLog, insert_audit_log};
use hammer_db::sqlx::PgTransaction;
use hammer_shared::{Id, audit as action, new_id};
use serde_json::{Map, Value};
use time::OffsetDateTime;

use crate::CoreError;

/// 不寫入操作紀錄的欄位。
pub const SENSITIVE_FIELDS: &[&str] = &["password_hash"];

/// 變更前後只包含有變動欄位的資料，對應 `before_data` / `after_data`。
///
/// - 新增（沒有變更前資料）：`before` 為 `None`，`after` 為全部欄位
/// - 刪除（沒有變更後資料）：`after` 為 `None`，`before` 為全部欄位
/// - 修改：兩邊都只有變動的欄位；只存在於一邊的欄位，另一邊記為 `null`
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AuditDiff {
    pub before: Option<Value>,
    pub after: Option<Value>,
}

impl AuditDiff {
    /// 比對變更前後的資料。兩者都必須是 JSON 物件（或 `None`），只比對第一層欄位。
    pub fn between(before: Option<&Value>, after: Option<&Value>) -> Result<Self, CoreError> {
        let before = before.map(as_object).transpose()?;
        let after = after.map(as_object).transpose()?;

        let (before, after) = match (before, after) {
            (None, None) => (None, None),
            (None, Some(a)) => (None, Some(without_sensitive(a))),
            (Some(b), None) => (Some(without_sensitive(b)), None),
            (Some(b), Some(a)) => {
                let mut changed_before = Map::new();
                let mut changed_after = Map::new();
                let keys = b.keys().chain(a.keys().filter(|k| !b.contains_key(*k)));
                for key in keys {
                    if is_sensitive(key) {
                        continue;
                    }
                    let old = b.get(key).unwrap_or(&Value::Null);
                    let new = a.get(key).unwrap_or(&Value::Null);
                    if old != new {
                        changed_before.insert(key.clone(), old.clone());
                        changed_after.insert(key.clone(), new.clone());
                    }
                }
                (Some(changed_before), Some(changed_after))
            }
        };

        Ok(Self {
            before: before.map(Value::Object),
            after: after.map(Value::Object),
        })
    }

    /// 修改時沒有任何欄位變動（排除敏感欄位後）。新增與刪除不算空。
    pub fn is_empty(&self) -> bool {
        fn empty(v: &Option<Value>) -> bool {
            v.as_ref()
                .and_then(Value::as_object)
                .is_some_and(Map::is_empty)
        }
        empty(&self.before) && empty(&self.after)
    }
}

fn as_object(value: &Value) -> Result<&Map<String, Value>, CoreError> {
    value
        .as_object()
        .ok_or_else(|| CoreError::Internal("操作紀錄的資料必須是 JSON 物件".into()))
}

fn is_sensitive(key: &str) -> bool {
    SENSITIVE_FIELDS.contains(&key)
}

fn without_sensitive(map: &Map<String, Value>) -> Map<String, Value> {
    map.iter()
        .filter(|(k, _)| !is_sensitive(k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// 由 HTTP 標頭組成 `client_info`：`{User-Agent} / {X-Client-Version}`。
/// 只有其中一個時只用那一個；都沒有（或都是空白）時為 `None`。
pub fn client_info(user_agent: Option<&str>, client_version: Option<&str>) -> Option<String> {
    let parts: Vec<&str> = [user_agent, client_version]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join(" / "))
}

/// 一筆要寫入的操作紀錄。
#[derive(Debug, Clone)]
pub struct AuditEntry<'a> {
    /// 操作者；帳號不存在的登入失敗為 `None`。
    pub user_id: Option<Id>,
    /// 動作代碼，必須是 [`hammer_shared::audit`] 中定義的值。
    pub action: &'a str,
    /// 資料類別代碼，例如 `user`。登入、登出等沒有對應資料的動作為 `None`。
    pub entity_key: Option<&'a str>,
    pub record_id: Option<Id>,
    pub diff: AuditDiff,
    /// 由呼叫端傳入，HTTP 層用 [`client_info`] 組成。
    pub client_info: Option<&'a str>,
    pub occurred_at: OffsetDateTime,
}

/// 在呼叫端的 transaction 內寫入一筆操作紀錄。
///
/// 必須與業務資料使用同一個 transaction：rollback 時紀錄也不會留下。
/// 回傳新紀錄的 id。
pub async fn record(tx: &mut PgTransaction<'_>, entry: &AuditEntry<'_>) -> Result<Id, CoreError> {
    if !action::is_valid(entry.action) {
        return Err(CoreError::Internal(format!(
            "未定義的操作紀錄動作：{}",
            entry.action
        )));
    }
    let id = new_id();
    insert_audit_log(
        &mut **tx,
        &NewAuditLog {
            id,
            occurred_at: entry.occurred_at,
            user_id: entry.user_id,
            action: entry.action,
            entity_key: entry.entity_key,
            record_id: entry.record_id,
            before_data: entry.diff.before.as_ref(),
            after_data: entry.diff.after.as_ref(),
            client_info: entry.client_info,
        },
    )
    .await?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn diff(before: Option<Value>, after: Option<Value>) -> AuditDiff {
        AuditDiff::between(before.as_ref(), after.as_ref()).unwrap()
    }

    #[test]
    fn update_keeps_only_changed_fields() {
        let d = diff(
            Some(json!({ "name": "A", "qty": "1.0000", "note": null, "tags": [1, 2] })),
            Some(json!({ "name": "B", "qty": "1.0000", "note": null, "tags": [1, 3] })),
        );
        assert_eq!(d.before, Some(json!({ "name": "A", "tags": [1, 2] })));
        assert_eq!(d.after, Some(json!({ "name": "B", "tags": [1, 3] })));
        assert!(!d.is_empty());
    }

    #[test]
    fn update_with_added_or_removed_fields() {
        let d = diff(
            Some(json!({ "a": 1, "b": 2 })),
            Some(json!({ "b": 2, "c": 3 })),
        );
        assert_eq!(d.before, Some(json!({ "a": 1, "c": null })));
        assert_eq!(d.after, Some(json!({ "a": null, "c": 3 })));
    }

    #[test]
    fn nested_values_are_compared_as_whole() {
        let d = diff(
            Some(json!({ "custom_data": { "x": 1, "y": 2 } })),
            Some(json!({ "custom_data": { "x": 1, "y": 3 } })),
        );
        assert_eq!(d.before, Some(json!({ "custom_data": { "x": 1, "y": 2 } })));
        assert_eq!(d.after, Some(json!({ "custom_data": { "x": 1, "y": 3 } })));
    }

    #[test]
    fn update_without_changes_is_empty() {
        let d = diff(Some(json!({ "a": 1 })), Some(json!({ "a": 1 })));
        assert_eq!(d.before, Some(json!({})));
        assert_eq!(d.after, Some(json!({})));
        assert!(d.is_empty());
    }

    #[test]
    fn create_has_null_before() {
        let d = diff(None, Some(json!({ "name": "A", "qty": 1 })));
        assert_eq!(d.before, None);
        assert_eq!(d.after, Some(json!({ "name": "A", "qty": 1 })));
        assert!(!d.is_empty());
    }

    #[test]
    fn delete_has_null_after() {
        let d = diff(Some(json!({ "name": "A" })), None);
        assert_eq!(d.before, Some(json!({ "name": "A" })));
        assert_eq!(d.after, None);
        assert!(!d.is_empty());
    }

    #[test]
    fn neither_side_is_all_null() {
        assert_eq!(diff(None, None), AuditDiff::default());
    }

    #[test]
    fn password_hash_never_appears() {
        let user = |hash: &str, name: &str| json!({ "username": "alice", "display_name": name, "password_hash": hash });
        let all = [
            diff(None, Some(user("h1", "A"))),
            diff(Some(user("h1", "A")), None),
            diff(Some(user("h1", "A")), Some(user("h2", "B"))),
            diff(Some(user("h1", "A")), Some(user("h2", "A"))),
        ];
        for d in &all {
            for side in [&d.before, &d.after].into_iter().flatten() {
                assert!(side.get("password_hash").is_none(), "{d:?}");
                assert!(!side.to_string().contains("h1") && !side.to_string().contains("h2"));
            }
        }
        // 只改密碼時，排除敏感欄位後沒有變動
        assert!(all[3].is_empty());
        assert_eq!(all[2].after, Some(json!({ "display_name": "B" })));
    }

    #[test]
    fn non_object_is_rejected() {
        let err = AuditDiff::between(Some(&json!([1])), None).unwrap_err();
        assert!(matches!(err, CoreError::Internal(_)));
        assert!(AuditDiff::between(None, Some(&json!("x"))).is_err());
    }

    #[test]
    fn client_info_combines_headers() {
        assert_eq!(
            client_info(Some("HammerERP/0.1 (macOS)"), Some("0.1.0")).as_deref(),
            Some("HammerERP/0.1 (macOS) / 0.1.0")
        );
        assert_eq!(client_info(Some("curl/8"), None).as_deref(), Some("curl/8"));
        assert_eq!(client_info(None, Some(" 0.1.0 ")).as_deref(), Some("0.1.0"));
        assert_eq!(client_info(Some("  "), None), None);
        assert_eq!(client_info(None, None), None);
    }
}

//! 操作紀錄查詢（docs/architecture.md §6.1 `audit.read`、§8.4 `audit_logs`、§10 分頁）。
//!
//! - [`list_audit_logs`]：檢查 `audit.read` 權限與參數後，依條件分頁查詢
//!
//! 寫入操作紀錄在 [`crate::audit`]。

use hammer_db::audit::{self as db, AuditLogFilter, AuditLogRow};
use hammer_db::sqlx::PgPool;
use hammer_shared::audit::{self as action, AuditLogEntry, AuditLogQuery, query_field};
use hammer_shared::pagination::{Page, PageRequest};
use hammer_shared::permission;

use crate::CoreError;
use crate::auth::AuthContext;

/// 檢查過的查詢條件。
#[derive(Debug, Clone, PartialEq, Eq)]
struct ValidQuery<'a> {
    filter: AuditLogFilter<'a>,
    page: PageRequest,
}

/// 依條件分頁查詢操作紀錄，依 `occurred_at` 由新到舊排序。需要 `audit.read`。
///
/// 參數不合法 → [`CoreError::InvalidField`]（`details.field` 為參數名稱）：
///
/// - `action` 不是已定義的動作代碼
/// - `from` 晚於 `to`
/// - `page` 或 `page_size` 小於 1
///
/// `page_size` 超過上限時套用上限（不視為錯誤）。空字串的文字條件視為未指定。
pub async fn list_audit_logs(
    pool: &PgPool,
    ctx: &AuthContext,
    query: &AuditLogQuery,
) -> Result<Page<AuditLogEntry>, CoreError> {
    ctx.require(permission::AUDIT_READ)?;
    let ValidQuery { filter, page } = validate(query)?;

    let total = db::count_audit_logs(pool, &filter).await?;
    let rows = db::list_audit_logs(pool, &filter, page.limit(), page.offset()).await?;

    let items = rows.into_iter().map(to_entry).collect();
    let total = u32::try_from(total).unwrap_or(u32::MAX);
    Ok(Page::new(items, total, page))
}

fn validate(query: &AuditLogQuery) -> Result<ValidQuery<'_>, CoreError> {
    let page =
        PageRequest::new(query.page, query.page_size).map_err(|err| CoreError::InvalidField {
            field: err.field,
            message: err.message().to_owned(),
        })?;

    let action = non_empty(query.action.as_deref());
    if let Some(code) = action
        && !action::is_valid(code)
    {
        return Err(CoreError::InvalidField {
            field: query_field::ACTION,
            message: format!("不支援的動作代碼：{code}"),
        });
    }

    if let (Some(from), Some(to)) = (query.from, query.to)
        && from > to
    {
        return Err(CoreError::InvalidField {
            field: query_field::FROM,
            message: "開始時間不可晚於結束時間".to_owned(),
        });
    }

    Ok(ValidQuery {
        filter: AuditLogFilter {
            from: query.from,
            to: query.to,
            user_id: query.user_id,
            action,
            entity_key: non_empty(query.entity_key.as_deref()),
            record_id: query.record_id,
        },
        page,
    })
}

/// 去除前後空白；空字串視為未指定。
fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|s| !s.is_empty())
}

fn to_entry(row: AuditLogRow) -> AuditLogEntry {
    AuditLogEntry {
        id: row.id,
        occurred_at: row.occurred_at,
        user_id: row.user_id,
        username: row.username,
        user_display_name: row.user_display_name,
        action: row.action,
        entity_key: row.entity_key,
        record_id: row.record_id,
        before_data: row.before_data,
        after_data: row.after_data,
        client_info: row.client_info,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hammer_shared::pagination::{DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE, field};
    use time::macros::datetime;

    fn invalid_field(result: Result<ValidQuery<'_>, CoreError>) -> (&'static str, String) {
        match result {
            Err(CoreError::InvalidField { field, message }) => (field, message),
            other => panic!("預期 InvalidField，實際為 {other:?}"),
        }
    }

    #[test]
    fn empty_query_uses_defaults() {
        let query = AuditLogQuery::default();
        let valid = validate(&query).unwrap();
        assert_eq!(valid.filter, AuditLogFilter::default());
        assert_eq!(valid.page, PageRequest::default());
        assert_eq!(valid.page.page_size, DEFAULT_PAGE_SIZE);
    }

    #[test]
    fn filters_are_passed_through() {
        let user_id = hammer_shared::new_id();
        let record_id = hammer_shared::new_id();
        let query = AuditLogQuery {
            from: Some(datetime!(2026-11-01 00:00 UTC)),
            to: Some(datetime!(2026-11-02 00:00 UTC)),
            user_id: Some(user_id),
            action: Some(" update ".into()),
            entity_key: Some("users".into()),
            record_id: Some(record_id),
            page: Some(3),
            page_size: Some(500),
        };
        let valid = validate(&query).unwrap();
        assert_eq!(
            valid.filter,
            AuditLogFilter {
                from: query.from,
                to: query.to,
                user_id: Some(user_id),
                action: Some(action::UPDATE),
                entity_key: Some("users"),
                record_id: Some(record_id),
            }
        );
        assert_eq!(valid.page.page, 3);
        assert_eq!(valid.page.page_size, MAX_PAGE_SIZE);
    }

    #[test]
    fn blank_text_filters_are_ignored() {
        let query = AuditLogQuery {
            action: Some(String::new()),
            entity_key: Some("  ".into()),
            ..Default::default()
        };
        let valid = validate(&query).unwrap();
        assert_eq!(valid.filter.action, None);
        assert_eq!(valid.filter.entity_key, None);
    }

    #[test]
    fn unknown_action_is_rejected() {
        let query = AuditLogQuery {
            action: Some("Login".into()),
            ..Default::default()
        };
        let (field, message) = invalid_field(validate(&query));
        assert_eq!(field, query_field::ACTION);
        assert!(message.contains("Login"), "{message}");
    }

    #[test]
    fn from_after_to_is_rejected() {
        let query = AuditLogQuery {
            from: Some(datetime!(2026-11-02 00:00 UTC)),
            to: Some(datetime!(2026-11-01 00:00 UTC)),
            ..Default::default()
        };
        assert_eq!(invalid_field(validate(&query)).0, query_field::FROM);

        // 相同時間不是錯誤（結果為空）
        let same = AuditLogQuery {
            to: query.from,
            ..query
        };
        assert!(validate(&same).is_ok());
    }

    #[test]
    fn invalid_page_is_rejected() {
        let query = AuditLogQuery {
            page: Some(0),
            ..Default::default()
        };
        assert_eq!(invalid_field(validate(&query)).0, field::PAGE);
        let query = AuditLogQuery {
            page_size: Some(0),
            ..Default::default()
        };
        assert_eq!(invalid_field(validate(&query)).0, field::PAGE_SIZE);
    }
}

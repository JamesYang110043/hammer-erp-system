//! 帳號管理（docs/architecture.md §6.1、§6.2、§8.4 `users`、§9.4）。
//!
//! 所有函式都先檢查 `user.manage` 權限，並在單一 transaction 內完成資料變更與操作紀錄。
//!
//! - [`list_roles`]、[`list_users`]、[`get_user`]：查詢
//! - [`create_user`]：建立帳號，`must_change_password = true`
//! - [`update_user`]：修改顯示名稱、角色、是否停用（樂觀鎖定）
//! - [`reset_password`]：設定臨時密碼、`must_change_password = true`、解除鎖定
//! - [`unlock_user`]：解除鎖定
//! - [`delete_user`]：軟刪除（樂觀鎖定）
//!
//! 停用、刪除、變更角色、重設密碼時，撤銷該使用者所有 session。
//!
//! **最後一個管理者保護**：會影響管理者人數的操作（修改、刪除）先取得 advisory lock
//! （[`ADMIN_GUARD_LOCK_KEY`]）排隊，在同一個 transaction 內完成變更後，再計算有效
//! （啟用、未刪除）的管理者人數；為 0 時 rollback 並回傳 [`CoreError::LastAdmin`]。
//! 排隊是為了避免兩位管理者同時停用對方，兩邊都以為還有另一位。

use hammer_db::lock::advisory_xact_lock;
use hammer_db::session::revoke_user_sessions;
use hammer_db::sqlx::{PgPool, PgTransaction};
use hammer_db::user::{
    self as db, NewUser, USERNAME_UNIQUE, UserRow, UserUpdate, count_active_admins,
    find_user_for_update, insert_user,
};
use hammer_db::{DbError, ensure_version_matched};
use hammer_shared::user::{Role, User};
use hammer_shared::{Id, audit as action, new_id, permission, role};
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::CoreError;
use crate::audit::{self, AuditDiff, AuditEntry};
use crate::auth::login::USER_ENTITY_KEY;
use crate::auth::{AuthContext, hash_password, validate_password_rules};

/// 修改、刪除帳號時使用的 advisory lock 鍵值，讓最後一個管理者的檢查依序進行。
pub const ADMIN_GUARD_LOCK_KEY: i64 = 0x4841_4d4d_4144_4d4e; // "HAMMADMN"

/// 帳號不存在或已刪除時的錯誤訊息。
pub const USER_NOT_FOUND: &str = "找不到此帳號";
/// 帳號重複時的錯誤訊息。
pub const USERNAME_TAKEN: &str = "此帳號已被使用";

/// 驗證錯誤 `details.field` 的欄位名稱，與 request 的欄位名稱相同。
pub mod field {
    pub const USERNAME: &str = "username";
    pub const DISPLAY_NAME: &str = "display_name";
    pub const ROLE: &str = "role";
    pub const PASSWORD: &str = "password";
    pub const NEW_PASSWORD: &str = "new_password";
}

// ---------------------------------------------------------------------------
// 輸入與純邏輯
// ---------------------------------------------------------------------------

/// 建立帳號的輸入。
#[derive(Clone, Copy)]
pub struct NewUserInput<'a> {
    pub username: &'a str,
    pub display_name: &'a str,
    /// 角色代碼。
    pub role: &'a str,
    /// 初始密碼。
    pub password: &'a str,
}

impl std::fmt::Debug for NewUserInput<'_> {
    /// 不印出密碼。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewUserInput")
            .field("username", &self.username)
            .field("display_name", &self.display_name)
            .field("role", &self.role)
            .field("password", &"***")
            .finish()
    }
}

impl<'a> NewUserInput<'a> {
    /// 檢查並整理輸入：帳號、顯示名稱去除前後空白後不可為空，角色必須是內建角色，
    /// 密碼符合 §6.2 規則。錯誤都標上欄位名稱。
    pub fn validate(&self) -> Result<NewUserInput<'a>, CoreError> {
        let username = self.username.trim();
        if username.is_empty() {
            return Err(invalid(field::USERNAME, "請輸入帳號"));
        }
        let display_name = validate_display_name(self.display_name)?;
        let role = validate_role(self.role)?;
        validate_password_rules(self.password).map_err(|e| e.with_field(field::PASSWORD))?;
        Ok(NewUserInput {
            username,
            display_name,
            role,
            password: self.password,
        })
    }
}

/// 修改帳號的輸入。`None` 的欄位不修改。
#[derive(Debug, Clone, Copy)]
pub struct UserChanges<'a> {
    /// 讀取時取得的版本號（§9.4）。
    pub version: i32,
    pub display_name: Option<&'a str>,
    /// 角色代碼。
    pub role: Option<&'a str>,
    pub is_active: Option<bool>,
}

/// 帳號管理可以修改的欄位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditableFields {
    pub display_name: String,
    /// 角色代碼。
    pub role: String,
    pub is_active: bool,
}

impl EditableFields {
    /// 由資料列取得目前的值。
    pub fn of(row: &UserRow) -> Self {
        Self {
            display_name: row.display_name.clone(),
            role: row.role_code.clone(),
            is_active: row.is_active,
        }
    }

    /// 寫入操作紀錄用的 JSON。
    fn to_json(&self) -> Value {
        json!({
            "display_name": self.display_name,
            "role": self.role,
            "is_active": self.is_active,
        })
    }
}

/// 檢查修改內容並與目前的值合併。沒有任何欄位改變時回傳 `None`。
pub fn plan_update(
    current: &EditableFields,
    changes: &UserChanges<'_>,
) -> Result<Option<EditableFields>, CoreError> {
    let display_name = match changes.display_name {
        Some(name) => validate_display_name(name)?.to_owned(),
        None => current.display_name.clone(),
    };
    let role = match changes.role {
        Some(code) => validate_role(code)?.to_owned(),
        None => current.role.clone(),
    };
    let next = EditableFields {
        display_name,
        role,
        is_active: changes.is_active.unwrap_or(current.is_active),
    };
    Ok((next != *current).then_some(next))
}

/// 修改後是否要撤銷該使用者所有 session：變更角色或停用。
pub fn revokes_sessions(before: &EditableFields, after: &EditableFields) -> bool {
    before.role != after.role || (before.is_active && !after.is_active)
}

/// 資料列轉成 API 回應。`is_locked` 依 `now` 判斷。
pub fn to_user(row: UserRow, now: OffsetDateTime) -> User {
    User {
        id: row.id,
        is_locked: row.locked_until.is_some_and(|until| until > now),
        username: row.username,
        display_name: row.display_name,
        role: row.role_code,
        role_name: row.role_name,
        is_active: row.is_active,
        locked_until: row.locked_until,
        must_change_password: row.must_change_password,
        last_login_at: row.last_login_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
        version: row.version,
    }
}

/// 版本號不符時回傳 [`DbError::Conflict`]（§9.4）。
pub fn ensure_version(current: i32, expected: i32) -> Result<(), CoreError> {
    if current == expected {
        Ok(())
    } else {
        Err(DbError::Conflict.into())
    }
}

/// 操作後的有效管理者人數為 0 時回傳 [`CoreError::LastAdmin`]。
pub fn ensure_admin_remains(active_admins: i64) -> Result<(), CoreError> {
    if active_admins > 0 {
        Ok(())
    } else {
        Err(CoreError::LastAdmin)
    }
}

fn validate_display_name(name: &str) -> Result<&str, CoreError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid(field::DISPLAY_NAME, "請輸入顯示名稱"));
    }
    Ok(name)
}

fn validate_role(code: &str) -> Result<&str, CoreError> {
    role::ALL
        .iter()
        .copied()
        .find(|r| *r == code)
        .ok_or_else(|| invalid(field::ROLE, "請選擇有效的角色"))
}

fn invalid(field: &'static str, message: &str) -> CoreError {
    CoreError::InvalidField {
        field,
        message: message.to_owned(),
    }
}

fn not_found() -> CoreError {
    CoreError::NotFound(USER_NOT_FOUND.into())
}

fn format_time(at: Option<OffsetDateTime>) -> Value {
    at.and_then(|t| t.format(&Rfc3339).ok())
        .map_or(Value::Null, Value::String)
}

/// 鎖定狀態，寫入解除鎖定與重設密碼的操作紀錄。
fn lock_state_json(row: &UserRow) -> Value {
    json!({
        "failed_login_count": row.failed_login_count,
        "locked_until": format_time(row.locked_until),
    })
}

// ---------------------------------------------------------------------------
// 查詢
// ---------------------------------------------------------------------------

/// 列出所有角色與其權限。
pub async fn list_roles(pool: &PgPool, ctx: &AuthContext) -> Result<Vec<Role>, CoreError> {
    ctx.require(permission::USER_MANAGE)?;
    let rows = hammer_db::role::list_roles(pool).await?;
    Ok(rows
        .into_iter()
        .map(|r| Role {
            id: r.id,
            code: r.code,
            name: r.name,
            permissions: r.permissions,
        })
        .collect())
}

/// 列出未刪除的帳號，依帳號排序。
pub async fn list_users(
    pool: &PgPool,
    ctx: &AuthContext,
    now: OffsetDateTime,
) -> Result<Vec<User>, CoreError> {
    ctx.require(permission::USER_MANAGE)?;
    let rows = db::list_users(pool).await?;
    Ok(rows.into_iter().map(|r| to_user(r, now)).collect())
}

/// 取得單一帳號。不存在或已刪除 → [`CoreError::NotFound`]。
pub async fn get_user(
    pool: &PgPool,
    ctx: &AuthContext,
    id: Id,
    now: OffsetDateTime,
) -> Result<User, CoreError> {
    ctx.require(permission::USER_MANAGE)?;
    let row = db::find_user(pool, id).await?.ok_or_else(not_found)?;
    Ok(to_user(row, now))
}

// ---------------------------------------------------------------------------
// 寫入
// ---------------------------------------------------------------------------

/// 建立帳號，`must_change_password = true`。
///
/// 帳號與既有帳號（含已刪除）重複 → [`CoreError::InvalidField`]（`username`）。
pub async fn create_user(
    pool: &PgPool,
    ctx: &AuthContext,
    input: &NewUserInput<'_>,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<User, CoreError> {
    ctx.require(permission::USER_MANAGE)?;
    let input = input.validate()?;
    let password_hash = hash_password(input.password)?;

    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let id = new_id();
    let inserted = insert_user(
        &mut *tx,
        &NewUser {
            id,
            username: input.username,
            display_name: input.display_name,
            password_hash: &password_hash,
            role_code: input.role,
            must_change_password: true,
            created_by: Some(ctx.user_id),
            created_at: now,
        },
    )
    .await
    .map_err(|e| {
        if e.unique_violation() == Some(USERNAME_UNIQUE) {
            invalid(field::USERNAME, USERNAME_TAKEN)
        } else {
            e.into()
        }
    })?;
    if !inserted {
        return Err(invalid(field::ROLE, "請選擇有效的角色"));
    }

    record(
        &mut tx,
        ctx,
        action::CREATE,
        id,
        AuditDiff::between(
            None,
            Some(&json!({
                "username": input.username,
                "display_name": input.display_name,
                "role": input.role,
                "is_active": true,
                "must_change_password": true,
            })),
        )?,
        client_info,
        now,
    )
    .await?;
    let row = fetch(&mut tx, id).await?;
    commit(tx).await?;

    tracing::info!(actor = %ctx.user_id, user_id = %id, role = input.role, "已建立帳號");
    Ok(to_user(row, now))
}

/// 修改顯示名稱、角色、是否停用。
///
/// - 不存在或已刪除 → [`CoreError::NotFound`]
/// - `version` 不符 → `Conflict`（§9.4）
/// - 操作後沒有有效的管理者 → [`CoreError::LastAdmin`]
/// - 沒有任何欄位改變時不寫入，回傳目前的資料（`version` 不變）
pub async fn update_user(
    pool: &PgPool,
    ctx: &AuthContext,
    id: Id,
    changes: &UserChanges<'_>,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<User, CoreError> {
    ctx.require(permission::USER_MANAGE)?;

    let mut tx = pool.begin().await.map_err(DbError::from)?;
    advisory_xact_lock(&mut *tx, ADMIN_GUARD_LOCK_KEY).await?;
    let current = find_user_for_update(&mut *tx, id)
        .await?
        .ok_or_else(not_found)?;
    ensure_version(current.version, changes.version)?;

    let before = EditableFields::of(&current);
    let Some(after) = plan_update(&before, changes)? else {
        return Ok(to_user(current, now));
    };

    ensure_version_matched(
        db::update_user(
            &mut *tx,
            &UserUpdate {
                id,
                version: changes.version,
                display_name: &after.display_name,
                role_code: &after.role,
                is_active: after.is_active,
                updated_by: ctx.user_id,
            },
        )
        .await?,
    )?;
    let revoked = if revokes_sessions(&before, &after) {
        revoke_user_sessions(&mut *tx, id, now).await?
    } else {
        0
    };
    ensure_admin_remains(count_active_admins(&mut *tx, role::ADMIN).await?)?;

    record(
        &mut tx,
        ctx,
        action::UPDATE,
        id,
        AuditDiff::between(Some(&before.to_json()), Some(&after.to_json()))?,
        client_info,
        now,
    )
    .await?;
    let row = fetch(&mut tx, id).await?;
    commit(tx).await?;

    tracing::info!(actor = %ctx.user_id, user_id = %id, revoked_sessions = revoked, "已修改帳號");
    Ok(to_user(row, now))
}

/// 管理者重設密碼：設定臨時密碼、`must_change_password = true`、解除鎖定，
/// 並撤銷該使用者所有 session。
///
/// 不需要 `version`：不會與其他人的編輯內容衝突，但會遞增 `version`。
pub async fn reset_password(
    pool: &PgPool,
    ctx: &AuthContext,
    id: Id,
    new_password: &str,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<User, CoreError> {
    ctx.require(permission::USER_MANAGE)?;
    validate_password_rules(new_password).map_err(|e| e.with_field(field::NEW_PASSWORD))?;
    let password_hash = hash_password(new_password)?;

    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let current = find_user_for_update(&mut *tx, id)
        .await?
        .ok_or_else(not_found)?;
    if !db::reset_password(&mut *tx, id, &password_hash, ctx.user_id).await? {
        return Err(not_found());
    }
    let revoked = revoke_user_sessions(&mut *tx, id, now).await?;
    let row = fetch(&mut tx, id).await?;

    let mut before = lock_state_json(&current);
    before["must_change_password"] = current.must_change_password.into();
    let mut after = lock_state_json(&row);
    after["must_change_password"] = row.must_change_password.into();
    record(
        &mut tx,
        ctx,
        action::PASSWORD_CHANGE,
        id,
        AuditDiff::between(Some(&before), Some(&after))?,
        client_info,
        now,
    )
    .await?;
    commit(tx).await?;

    tracing::info!(actor = %ctx.user_id, user_id = %id, revoked_sessions = revoked, "已重設密碼");
    Ok(to_user(row, now))
}

/// 解除鎖定：清除登入失敗次數與鎖定時間（不遞增 `version`）。
///
/// 原本就沒有失敗紀錄時不寫入，也不留操作紀錄。
pub async fn unlock_user(
    pool: &PgPool,
    ctx: &AuthContext,
    id: Id,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<User, CoreError> {
    ctx.require(permission::USER_MANAGE)?;

    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let current = find_user_for_update(&mut *tx, id)
        .await?
        .ok_or_else(not_found)?;
    if current.failed_login_count == 0 && current.locked_until.is_none() {
        return Ok(to_user(current, now));
    }
    if !db::unlock_user(&mut *tx, id).await? {
        return Err(not_found());
    }
    let row = fetch(&mut tx, id).await?;
    record(
        &mut tx,
        ctx,
        action::UPDATE,
        id,
        AuditDiff::between(
            Some(&lock_state_json(&current)),
            Some(&lock_state_json(&row)),
        )?,
        client_info,
        now,
    )
    .await?;
    commit(tx).await?;

    tracing::info!(actor = %ctx.user_id, user_id = %id, "已解除鎖定");
    Ok(to_user(row, now))
}

/// 軟刪除帳號，並撤銷該使用者所有 session。
///
/// 帳號名稱仍保留，不可再用來建立新帳號。錯誤同 [`update_user`]。
pub async fn delete_user(
    pool: &PgPool,
    ctx: &AuthContext,
    id: Id,
    version: i32,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<(), CoreError> {
    ctx.require(permission::USER_MANAGE)?;

    let mut tx = pool.begin().await.map_err(DbError::from)?;
    advisory_xact_lock(&mut *tx, ADMIN_GUARD_LOCK_KEY).await?;
    let current = find_user_for_update(&mut *tx, id)
        .await?
        .ok_or_else(not_found)?;
    ensure_version(current.version, version)?;

    ensure_version_matched(db::soft_delete_user(&mut *tx, id, version, now, ctx.user_id).await?)?;
    let revoked = revoke_user_sessions(&mut *tx, id, now).await?;
    ensure_admin_remains(count_active_admins(&mut *tx, role::ADMIN).await?)?;

    let mut before = EditableFields::of(&current).to_json();
    before["username"] = current.username.clone().into();
    record(
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

    tracing::info!(actor = %ctx.user_id, user_id = %id, revoked_sessions = revoked, "已刪除帳號");
    Ok(())
}

async fn record(
    tx: &mut PgTransaction<'_>,
    ctx: &AuthContext,
    action: &str,
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
            entity_key: Some(USER_ENTITY_KEY),
            record_id: Some(record_id),
            diff,
            client_info,
            occurred_at: now,
        },
    )
    .await?;
    Ok(())
}

async fn fetch(tx: &mut PgTransaction<'_>, id: Id) -> Result<UserRow, CoreError> {
    db::find_user(&mut **tx, id)
        .await?
        .ok_or_else(|| CoreError::Internal("寫入後找不到帳號".into()))
}

async fn commit(tx: PgTransaction<'_>) -> Result<(), CoreError> {
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hammer_shared::ErrorCode;
    use time::Duration;
    use time::macros::datetime;

    const PASSWORD: &str = "correct horse battery";
    const NOW: OffsetDateTime = datetime!(2026-11-13 09:00 +8);

    fn input<'a>(username: &'a str, display_name: &'a str, role: &'a str) -> NewUserInput<'a> {
        NewUserInput {
            username,
            display_name,
            role,
            password: PASSWORD,
        }
    }

    fn field_of(err: &CoreError) -> &'static str {
        match err {
            CoreError::InvalidField { field, .. } => field,
            other => panic!("不是欄位錯誤：{other:?}"),
        }
    }

    fn fields(role: &str, is_active: bool) -> EditableFields {
        EditableFields {
            display_name: "Alice".into(),
            role: role.into(),
            is_active,
        }
    }

    fn changes<'a>(
        display_name: Option<&'a str>,
        role: Option<&'a str>,
        is_active: Option<bool>,
    ) -> UserChanges<'a> {
        UserChanges {
            version: 1,
            display_name,
            role,
            is_active,
        }
    }

    fn row() -> UserRow {
        UserRow {
            id: new_id(),
            username: "alice".into(),
            display_name: "Alice".into(),
            role_code: role::EDITOR.into(),
            role_name: "可編輯者".into(),
            is_active: true,
            must_change_password: false,
            failed_login_count: 0,
            locked_until: None,
            last_login_at: None,
            created_at: NOW,
            updated_at: NOW,
            version: 1,
        }
    }

    #[test]
    fn valid_input_is_trimmed() {
        let v = input(" bob ", " 小明\t", role::VIEWER).validate().unwrap();
        assert_eq!(v.username, "bob");
        assert_eq!(v.display_name, "小明");
        assert_eq!(v.role, role::VIEWER);
        assert_eq!(v.password, PASSWORD);
    }

    #[test]
    fn invalid_input_reports_field() {
        let cases = [
            (input(" ", "A", role::VIEWER), field::USERNAME),
            (input("bob", "", role::VIEWER), field::DISPLAY_NAME),
            (input("bob", "A", "superuser"), field::ROLE),
            (input("bob", "A", ""), field::ROLE),
            (
                NewUserInput {
                    password: "123456789",
                    ..input("bob", "A", role::VIEWER)
                },
                field::PASSWORD,
            ),
        ];
        for (input, expected) in cases {
            let err = input.validate().unwrap_err();
            assert_eq!(err.code(), ErrorCode::Validation);
            assert_eq!(field_of(&err), expected, "{input:?}");
        }
    }

    #[test]
    fn input_debug_hides_password() {
        let debug = format!("{:?}", input("bob", "A", role::VIEWER));
        assert!(debug.contains("bob") && !debug.contains(PASSWORD));
    }

    #[test]
    fn plan_merges_changes() {
        let current = fields(role::EDITOR, true);
        let next = plan_update(&current, &changes(Some(" Bob "), None, Some(false)))
            .unwrap()
            .unwrap();
        assert_eq!(
            next,
            EditableFields {
                display_name: "Bob".into(),
                role: role::EDITOR.into(),
                is_active: false,
            }
        );
    }

    #[test]
    fn plan_without_changes_is_none() {
        let current = fields(role::EDITOR, true);
        assert_eq!(
            plan_update(&current, &changes(None, None, None)).unwrap(),
            None
        );
        // 與目前的值相同也算沒有改變
        assert_eq!(
            plan_update(
                &current,
                &changes(Some("Alice "), Some(role::EDITOR), Some(true))
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn plan_rejects_invalid_values() {
        let current = fields(role::EDITOR, true);
        let err = plan_update(&current, &changes(Some("  "), None, None)).unwrap_err();
        assert_eq!(field_of(&err), field::DISPLAY_NAME);
        let err = plan_update(&current, &changes(None, Some("root"), None)).unwrap_err();
        assert_eq!(field_of(&err), field::ROLE);
    }

    #[test]
    fn sessions_revoked_on_role_change_or_deactivation() {
        let editor = fields(role::EDITOR, true);
        assert!(revokes_sessions(&editor, &fields(role::VIEWER, true)));
        assert!(revokes_sessions(&editor, &fields(role::EDITOR, false)));
        // 只改顯示名稱、或重新啟用，不撤銷
        let renamed = EditableFields {
            display_name: "Bob".into(),
            ..editor.clone()
        };
        assert!(!revokes_sessions(&editor, &renamed));
        assert!(!revokes_sessions(&fields(role::EDITOR, false), &editor));
    }

    #[test]
    fn to_user_computes_lock_state() {
        let mut r = row();
        assert!(!to_user(r.clone(), NOW).is_locked);
        r.locked_until = Some(NOW + Duration::minutes(5));
        let user = to_user(r.clone(), NOW);
        assert!(user.is_locked);
        assert_eq!(user.locked_until, r.locked_until);
        // 鎖定時間已過
        assert!(!to_user(r, NOW + Duration::minutes(5)).is_locked);
    }

    #[test]
    fn to_user_copies_fields() {
        let r = row();
        let user = to_user(r.clone(), NOW);
        assert_eq!(user.id, r.id);
        assert_eq!(user.username, "alice");
        assert_eq!(user.role, role::EDITOR);
        assert_eq!(user.role_name, "可編輯者");
        assert_eq!(user.version, 1);
    }

    #[test]
    fn version_mismatch_is_conflict() {
        assert!(ensure_version(3, 3).is_ok());
        assert_eq!(
            ensure_version(3, 2).unwrap_err().code(),
            ErrorCode::Conflict
        );
    }

    #[test]
    fn last_admin_check() {
        assert!(ensure_admin_remains(1).is_ok());
        assert!(matches!(ensure_admin_remains(0), Err(CoreError::LastAdmin)));
    }

    #[test]
    fn lock_state_formats_time() {
        let mut r = row();
        r.failed_login_count = 5;
        r.locked_until = Some(datetime!(2026-11-13 01:05:30 UTC));
        assert_eq!(
            lock_state_json(&r),
            json!({ "failed_login_count": 5, "locked_until": "2026-11-13T01:05:30Z" })
        );
        assert_eq!(
            lock_state_json(&row()),
            json!({ "failed_login_count": 0, "locked_until": null })
        );
    }

    #[test]
    fn errors_have_expected_codes() {
        assert_eq!(not_found().code(), ErrorCode::NotFound);
        assert_eq!(not_found().to_string(), USER_NOT_FOUND);
    }
}

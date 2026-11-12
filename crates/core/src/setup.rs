//! 初次啟動：資料庫沒有任何帳號時，建立第一個管理者（docs/architecture.md §11）。
//!
//! - [`needs_setup`]：`users` 沒有任何資料（含已停用、已刪除）時為 `true`
//! - [`create_initial_admin`]：建立管理者並直接登入
//!
//! 同時有兩個請求建立時，以 advisory lock（[`SETUP_LOCK_KEY`]）排隊，
//! 在鎖內再確認一次沒有帳號，因此只有一個會成功，另一個回傳 [`CoreError::Conflict`]。
//!
//! 開放多人連線之後是否要限制只能從本機呼叫，等 §13 Q4 決定。

use hammer_db::DbError;
use hammer_db::lock::advisory_xact_lock;
use hammer_db::sqlx::PgPool;
use hammer_db::user::{NewUser, any_user_exists, find_auth_user, insert_user};
use hammer_shared::{audit as action, new_id, role};
use serde_json::json;
use time::OffsetDateTime;

use crate::CoreError;
use crate::audit::{self, AuditDiff, AuditEntry};
use crate::auth::login::USER_ENTITY_KEY;
use crate::auth::validate_password_rules;
use crate::auth::{AuthContext, LoginOutcome, SecuritySettings, hash_password, session};

/// 初次啟動建立管理者時使用的 advisory lock 鍵值（`pg_advisory_xact_lock`）。
pub const SETUP_LOCK_KEY: i64 = 0x4841_4d4d_5345_5455; // "HAMMSETU"

/// 已有帳號時的錯誤訊息。
pub const ALREADY_SET_UP: &str = "系統已完成初始設定，請直接登入";

/// 建立第一個管理者的輸入。
#[derive(Clone, Copy)]
pub struct InitialAdmin<'a> {
    pub username: &'a str,
    pub display_name: &'a str,
    pub password: &'a str,
}

impl std::fmt::Debug for InitialAdmin<'_> {
    /// 不印出密碼。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InitialAdmin")
            .field("username", &self.username)
            .field("display_name", &self.display_name)
            .field("password", &"***")
            .finish()
    }
}

impl<'a> InitialAdmin<'a> {
    /// 檢查並整理輸入：帳號與顯示名稱去除前後空白後不可為空，密碼符合 §6.2 規則。
    ///
    /// 回傳去除空白後的帳號與顯示名稱；密碼不做任何修改。
    pub fn validate(&self) -> Result<InitialAdmin<'a>, CoreError> {
        let username = self.username.trim();
        if username.is_empty() {
            return Err(CoreError::Validation("請輸入帳號".into()));
        }
        let display_name = self.display_name.trim();
        if display_name.is_empty() {
            return Err(CoreError::Validation("請輸入顯示名稱".into()));
        }
        validate_password_rules(self.password)?;
        Ok(InitialAdmin {
            username,
            display_name,
            password: self.password,
        })
    }
}

/// 是否需要初次設定：`users` 沒有任何資料。
pub async fn needs_setup(pool: &PgPool) -> Result<bool, CoreError> {
    Ok(!any_user_exists(pool).await?)
}

/// 建立第一個管理者並登入，回傳 token 與使用者資訊。
///
/// - 已有任何帳號 → [`CoreError::Conflict`]（先於輸入檢查判斷）
/// - 輸入不符合規則 → [`CoreError::Validation`]
/// - 建立的帳號 `must_change_password = false`（密碼是使用者自己設定的）
/// - 在同一個 transaction 內寫入 `create` 操作紀錄（不含密碼）
pub async fn create_initial_admin(
    pool: &PgPool,
    input: &InitialAdmin<'_>,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<LoginOutcome, CoreError> {
    // 已設定過就不做密碼雜湊等耗時的工作
    if !needs_setup(pool).await? {
        return Err(already_set_up());
    }
    let input = input.validate()?;
    let password_hash = hash_password(input.password)?;
    let settings = SecuritySettings::load(pool).await?;

    let mut tx = pool.begin().await.map_err(DbError::from)?;
    advisory_xact_lock(&mut *tx, SETUP_LOCK_KEY).await?;
    if any_user_exists(&mut *tx).await? {
        return Err(already_set_up());
    }

    let id = new_id();
    let inserted = insert_user(
        &mut *tx,
        &NewUser {
            id,
            username: input.username,
            display_name: input.display_name,
            password_hash: &password_hash,
            role_code: role::ADMIN,
            must_change_password: false,
            created_by: None,
            created_at: now,
        },
    )
    .await?;
    if !inserted {
        return Err(CoreError::Internal("找不到內建的管理者角色".into()));
    }

    let (token, session_row) = session::create(&mut *tx, id, client_info, now, &settings).await?;
    audit::record(
        &mut tx,
        &AuditEntry {
            user_id: Some(id),
            action: action::CREATE,
            entity_key: Some(USER_ENTITY_KEY),
            record_id: Some(id),
            diff: AuditDiff::between(
                None,
                Some(&json!({
                    "username": input.username,
                    "display_name": input.display_name,
                    "role": role::ADMIN,
                    "must_change_password": false,
                })),
            )?,
            client_info,
            occurred_at: now,
        },
    )
    .await?;
    let row = find_auth_user(&mut *tx, id)
        .await?
        .ok_or_else(|| CoreError::Internal("建立後找不到管理者".into()))?;
    tx.commit().await.map_err(DbError::from)?;

    tracing::info!(user_id = %id, session_id = %session_row.id, "已建立第一個管理者");
    Ok(LoginOutcome {
        token,
        user: AuthContext::new(row, session_row.id),
    })
}

fn already_set_up() -> CoreError {
    CoreError::Conflict(ALREADY_SET_UP.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hammer_shared::ErrorCode;

    const PASSWORD: &str = "correct horse battery";

    fn input<'a>(username: &'a str, display_name: &'a str, password: &'a str) -> InitialAdmin<'a> {
        InitialAdmin {
            username,
            display_name,
            password,
        }
    }

    #[test]
    fn valid_input_is_trimmed() {
        let v = input("  admin ", " 管理者\t", PASSWORD).validate().unwrap();
        assert_eq!(v.username, "admin");
        assert_eq!(v.display_name, "管理者");
        assert_eq!(v.password, PASSWORD);
    }

    #[test]
    fn password_is_not_trimmed() {
        let v = input("admin", "A", "  ten chars ").validate().unwrap();
        assert_eq!(v.password, "  ten chars ");
    }

    #[test]
    fn empty_username_is_rejected() {
        for name in ["", "   "] {
            let err = input(name, "A", PASSWORD).validate().unwrap_err();
            assert_eq!(err.code(), ErrorCode::Validation);
            assert_eq!(err.to_string(), "請輸入帳號");
        }
    }

    #[test]
    fn empty_display_name_is_rejected() {
        let err = input("admin", " ", PASSWORD).validate().unwrap_err();
        assert_eq!(err.code(), ErrorCode::Validation);
        assert_eq!(err.to_string(), "請輸入顯示名稱");
    }

    #[test]
    fn short_password_is_rejected() {
        let err = input("admin", "A", "123456789").validate().unwrap_err();
        assert_eq!(err.code(), ErrorCode::Validation);
        assert_eq!(err.to_string(), "密碼至少需要 10 個字元");
    }

    #[test]
    fn debug_hides_password() {
        let debug = format!("{:?}", input("admin", "A", PASSWORD));
        assert!(debug.contains("admin"));
        assert!(!debug.contains(PASSWORD));
    }

    #[test]
    fn already_set_up_is_conflict() {
        let err = already_set_up();
        assert_eq!(err.code(), ErrorCode::Conflict);
        assert_eq!(err.to_string(), ALREADY_SET_UP);
    }
}

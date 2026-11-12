//! 整合測試輔助工具：直接在資料庫建立使用者與 session，不經過登入 API。
//!
//! 只在啟用 `test-util` feature 時編譯，給各 crate 的 `[dev-dependencies]` 使用：
//!
//! ```toml
//! hammer-core = { workspace = true, features = ["test-util"] }
//! ```
//!
//! ```ignore
//! let user_id = TestUser::new("alice", role::EDITOR).insert(&pool).await?;
//! let token = create_session(&pool, user_id).await?;
//! // 請求帶上 `Authorization: Bearer {token.as_str()}`
//! ```

use hammer_db::sqlx::{self, PgPool};
use hammer_shared::{Id, new_id};
use time::OffsetDateTime;

use crate::CoreError;
use crate::auth::{SecuritySettings, SessionToken, hash_password, session};

/// 要建立的測試帳號。預設為啟用、不需改密碼、沒有可用的密碼（無法透過登入 API 登入）。
#[derive(Debug, Clone)]
pub struct TestUser<'a> {
    pub username: &'a str,
    /// 角色代碼，對應 `hammer_shared::role`。
    pub role: &'a str,
    /// 明文密碼。`None` 時存入無效的雜湊，任何密碼都無法登入。
    pub password: Option<&'a str>,
    pub is_active: bool,
    pub must_change_password: bool,
}

impl<'a> TestUser<'a> {
    pub fn new(username: &'a str, role: &'a str) -> Self {
        Self {
            username,
            role,
            password: None,
            is_active: true,
            must_change_password: false,
        }
    }

    /// 設定可登入的密碼（Argon2id 雜湊後存入）。
    pub fn password(mut self, password: &'a str) -> Self {
        self.password = Some(password);
        self
    }

    /// 設定為停用。
    pub fn inactive(mut self) -> Self {
        self.is_active = false;
        self
    }

    /// 設定為下次登入必須修改密碼。
    pub fn must_change_password(mut self) -> Self {
        self.must_change_password = true;
        self
    }

    /// 寫入資料庫，回傳使用者 id。角色代碼不存在時回傳錯誤。
    pub async fn insert(&self, pool: &PgPool) -> Result<Id, CoreError> {
        let password_hash = match self.password {
            Some(p) => hash_password(p)?,
            None => "!".to_owned(),
        };
        let id = new_id();
        let inserted = sqlx::query(
            "INSERT INTO users
                 (id, username, display_name, password_hash, role_id, is_active, must_change_password)
             SELECT $1, $2, $2, $3, id, $5, $6 FROM roles WHERE code = $4",
        )
        .bind(id)
        .bind(self.username)
        .bind(password_hash)
        .bind(self.role)
        .bind(self.is_active)
        .bind(self.must_change_password)
        .execute(pool)
        .await
        .map_err(hammer_db::DbError::from)?
        .rows_affected();
        if inserted == 0 {
            return Err(CoreError::Internal(format!("角色不存在：{}", self.role)));
        }
        Ok(id)
    }
}

/// 為使用者建立一個目前有效的 session（使用預設安全設定），回傳 token。
pub async fn create_session(pool: &PgPool, user_id: Id) -> Result<SessionToken, CoreError> {
    let (token, _) = session::create(
        pool,
        user_id,
        Some("test"),
        OffsetDateTime::now_utc(),
        &SecuritySettings::default(),
    )
    .await?;
    Ok(token)
}

/// 建立使用者並登入，回傳使用者 id 與 token。
pub async fn create_user_with_session(
    pool: &PgPool,
    user: &TestUser<'_>,
) -> Result<(Id, SessionToken), CoreError> {
    let id = user.insert(pool).await?;
    let token = create_session(pool, id).await?;
    Ok((id, token))
}

//! 認證：密碼、session token、session 有效判斷、登入鎖定（docs/architecture.md §6.2）。
//!
//! - [`password`]：Argon2id 雜湊與驗證、密碼規則
//! - [`token`]：session token 的產生與 SHA-256 雜湊
//! - [`settings`]：型別化的安全設定 [`SecuritySettings`]
//! - [`lockout`]：連續登入失敗的鎖定判斷
//! - [`session`]：session 有效判斷，以及建立、查詢、更新、撤銷
//! - [`context`]：由 token 取得目前使用者（[`authenticate`]）與權限檢查（[`AuthContext`]）
//!
//! HTTP 端點、extractor 與操作紀錄不在這裡。

pub mod context;
pub mod lockout;
pub mod password;
pub mod session;
pub mod settings;
pub mod token;

pub use context::{AuthContext, authenticate};
pub use lockout::LoginAttemptState;
pub use password::{hash_password, validate_password_rules, verify_password};
pub use session::{SessionStatus, session_status};
pub use settings::SecuritySettings;
pub use token::{SessionToken, hash_token};

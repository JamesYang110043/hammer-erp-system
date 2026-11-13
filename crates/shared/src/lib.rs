//! 前後端共用：資料型別、錯誤碼、權限常數。
//!
//! API Server 與 Tauri 端都依賴本 crate，確保兩邊的定義一致。
//! 本 crate 位於相依方向的最底層，不可依賴專案內其他 crate（`docs/architecture.md` §4）。

pub mod audit;
pub mod auth;
pub mod constants;
pub mod decimal;
pub mod error;
pub mod health;
pub mod id;
pub mod layout;
pub mod pagination;
pub mod permission;
pub mod record;
pub mod role;
pub mod schema;
pub mod setup;
pub mod time;
pub mod user;

pub use error::{ApiError, ErrorCode};
pub use id::{Id, new_id};

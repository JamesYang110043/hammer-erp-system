//! 商業邏輯：單據流程、庫存 FIFO、匯率換算、權限判斷。
//!
//! 目前只是骨架，尚無實作。相依方向見 `docs/architecture.md` §4。

/// 重新匯出資料存取層。`server` 只依賴 `core`（§4），
/// 連線池與 migration 透過這裡取得，不直接依賴 `hammer-db`。
pub use hammer_db as db;

#[cfg(test)]
mod tests {
    #[test]
    fn crate_compiles() {}
}

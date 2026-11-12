//! 新增或修改 migration 檔案時重新編譯，讓 `sqlx::migrate!` 嵌入最新內容。
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}

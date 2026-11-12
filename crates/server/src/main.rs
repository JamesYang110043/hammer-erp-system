//! Hammer ERP API Server 執行檔。

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    hammer_server::run().await
}

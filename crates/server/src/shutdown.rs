//! Graceful shutdown 訊號。

/// 等待 Ctrl+C 或 SIGTERM（Unix）。回傳後 axum 會停止接受新連線，
/// 並等進行中的請求完成再結束。
pub async fn signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::error!(error = %err, "無法監聽 Ctrl+C");
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(err) => {
                tracing::error!(error = %err, "無法監聽 SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => tracing::info!("收到 Ctrl+C，開始關閉"),
        () = terminate => tracing::info!("收到 SIGTERM，開始關閉"),
    }
}

//! tracing log 初始化。

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;

use crate::config::{Config, LogFormat};

/// 依設定初始化全域 tracing subscriber。只能呼叫一次。
pub fn init(config: &Config) -> anyhow::Result<()> {
    let filter = EnvFilter::try_new(&config.log_level)
        .map_err(|err| anyhow::anyhow!("log 等級「{}」無效：{err}", config.log_level))?;

    let builder = fmt().with_env_filter(filter).with_target(true);
    let result = match config.log_format {
        LogFormat::Pretty => builder.try_init(),
        LogFormat::Json => builder
            .json()
            .flatten_event(true)
            .with_current_span(true)
            .with_span_list(false)
            .try_init(),
    };
    result.map_err(|err| anyhow::anyhow!(err))
}

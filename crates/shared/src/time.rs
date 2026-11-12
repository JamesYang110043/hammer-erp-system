//! 時間處理（`docs/architecture.md` §5、D13）。
//!
//! 全專案統一使用 `time` crate（不使用 `chrono`）。
//!
//! - 時間點用 [`OffsetDateTime`]，對應 PostgreSQL `TIMESTAMPTZ`，JSON 以 RFC 3339 字串傳遞。
//! - 日期用 [`Date`]，對應 `DATE`。
//! - 系統時區 Asia/Taipei 固定為 UTC+8（台灣自 1979 年後不實施日光節約），
//!   因此用固定位移 [`TAIPEI_OFFSET`]，不需要時區資料庫。

pub use ::time::{Date, OffsetDateTime, UtcOffset};

/// Asia/Taipei 的 UTC 位移（+08:00）。
pub const TAIPEI_OFFSET: UtcOffset = ::time::macros::offset!(+8);

/// 目前時間（Asia/Taipei 位移）。
pub fn now_taipei() -> OffsetDateTime {
    OffsetDateTime::now_utc().to_offset(TAIPEI_OFFSET)
}

/// 今天的日期（以 Asia/Taipei 計算）。
pub fn today_taipei() -> Date {
    now_taipei().date()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::time::macros::datetime;

    #[test]
    fn utc_converts_to_taipei_date() {
        // UTC 16:30 已是台北隔天 00:30
        let utc = datetime!(2026-01-01 16:30 UTC);
        let taipei = utc.to_offset(TAIPEI_OFFSET);
        assert_eq!(taipei.date(), ::time::macros::date!(2026 - 01 - 02));
        assert_eq!(taipei.offset(), TAIPEI_OFFSET);
    }

    #[test]
    fn now_taipei_has_taipei_offset() {
        assert_eq!(now_taipei().offset().whole_hours(), 8);
    }
}

//! 樂觀鎖定輔助函式（docs/architecture.md D12、§9.4）。
//!
//! 更新語句的寫法：
//!
//! ```sql
//! UPDATE <table> SET …, version = version + 1
//! WHERE id = $1 AND version = $2
//! ```
//!
//! 執行後把結果交給 [`ensure_version_matched`]：影響 0 筆代表版本號不符
//! （資料已被其他人修改），回傳 [`DbError::Conflict`]。
//!
//! 注意：資料不存在（或已軟刪除而被 WHERE 排除）時同樣影響 0 筆，也會回傳
//! `Conflict`。需要區分「找不到」的情況，由呼叫端先查詢資料是否存在。

use sqlx::postgres::PgQueryResult;

use crate::DbError;

/// 檢查帶版本條件的 UPDATE 是否有更新到資料。
pub fn ensure_version_matched(result: PgQueryResult) -> Result<PgQueryResult, DbError> {
    check_rows_affected(result.rows_affected())?;
    Ok(result)
}

fn check_rows_affected(rows_affected: u64) -> Result<(), DbError> {
    if rows_affected == 0 {
        Err(DbError::Conflict)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_rows_is_conflict() {
        assert!(check_rows_affected(0).unwrap_err().is_conflict());
    }

    #[test]
    fn one_row_is_ok() {
        assert!(check_rows_affected(1).is_ok());
    }
}

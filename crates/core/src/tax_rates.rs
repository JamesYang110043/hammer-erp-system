//! 稅別（docs/architecture.md §8.6 `tax_rates`）。M3 只提供查詢。

use hammer_db::sqlx::PgPool;
use hammer_db::tax_rate::{self as db, TaxRateRow};
use hammer_shared::tax_rate::TaxRate;

use crate::CoreError;
use crate::auth::AuthContext;

/// 列出所有稅別（依稅率由高到低、再依代碼排序）。需要登入，不需要特定權限。
pub async fn list_tax_rates(pool: &PgPool, _ctx: &AuthContext) -> Result<Vec<TaxRate>, CoreError> {
    let rows = db::list_tax_rates(pool).await?;
    Ok(rows.into_iter().map(to_tax_rate).collect())
}

fn to_tax_rate(row: TaxRateRow) -> TaxRate {
    TaxRate {
        code: row.code,
        name: row.name,
        rate: row.rate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hammer_shared::decimal::Decimal;

    #[test]
    fn row_is_copied() {
        let tax = to_tax_rate(TaxRateRow {
            code: "taxable".into(),
            name: "應稅".into(),
            rate: Decimal::new(500, 4),
        });
        assert_eq!(tax.code, "taxable");
        assert_eq!(tax.name, "應稅");
        assert_eq!(tax.rate.to_string(), "0.0500");
    }
}

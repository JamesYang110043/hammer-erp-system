//! 發票模組（銷項發票、銷貨折讓、進項憑證、進貨折讓，docs/architecture.md §8.11）的專屬讀取。
//!
//! 一般的新增、修改走通用資料 API（`system_record`）；這裡是存檔時需要的、跨單據的讀取。
//! 所有函式都在呼叫端的 transaction 內執行（傳入 `&mut *tx`）。
//!
//! - [`find_sales_invoice_for_allowance`]：銷貨折讓帶入表頭用的銷項發票（`FOR SHARE`）
//! - [`find_purchase_invoice_for_allowance`]：進貨折讓帶入表頭用的進項憑證（`FOR SHARE`）
//! - 銷項發票：帶入買受人的客戶資料、關聯出貨單的讀取與鎖定、已開立發票的出貨單、號碼是否已使用、
//!   有效的折讓
//! - 銷貨折讓：鎖定原發票、關聯的銷貨退回、已開立折讓的合計
//! - 進項憑證、進貨折讓：廠商統編、關聯進貨單的讀取與鎖定、已登錄憑證的進貨單（發票類與海關繳納證分開）、
//!   號碼是否已登錄、有效的折讓、關聯的進貨退回
//! - [`set_void_details`]：作廢日期與原因
//! - [`count_customer_invoice_documents`]、[`count_supplier_invoice_documents`]：客戶、廠商被引用的張數

use rust_decimal::Decimal;
use sqlx::PgExecutor;
use time::Date;
use uuid::Uuid;

use crate::DbError;

/// 折讓從原發票（憑證）帶入的表頭值（§8.11「銷貨折讓」「進貨折讓」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvoiceForAllowance {
    pub id: Uuid,
    /// `draft` / `posted` / `void`。
    pub status: String,
    /// 銷項發票為客戶、進項憑證為廠商。
    pub party_id: Uuid,
    pub tax_type: String,
    /// 銷項發票為 `triplicate` / `duplicate`；進項憑證為 `triplicate` / `electronic` / `customs`。
    pub doc_type: String,
    /// 發票（憑證）日期。
    pub invoice_date: Date,
    /// 發票（憑證）總計。
    pub total: Decimal,
}

/// 未刪除的銷項發票 `id`，以 `FOR SHARE` 鎖定（存檔期間不會被開立、作廢或刪除）。不存在或已刪除時為
/// `None`。
pub async fn find_sales_invoice_for_allowance<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
) -> Result<Option<InvoiceForAllowance>, DbError> {
    let row = sqlx::query_as!(
        InvoiceForAllowance,
        r#"
        SELECT id, status, customer_id AS party_id, tax_type, invoice_type AS doc_type,
               invoice_date, total
        FROM sales_invoices
        WHERE id = $1 AND deleted_at IS NULL
        FOR SHARE
        "#,
        id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 未刪除的進項憑證 `id`，以 `FOR SHARE` 鎖定。不存在或已刪除時為 `None`。
pub async fn find_purchase_invoice_for_allowance<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
) -> Result<Option<InvoiceForAllowance>, DbError> {
    let row = sqlx::query_as!(
        InvoiceForAllowance,
        r#"
        SELECT id, status, supplier_id AS party_id, tax_type, doc_type, invoice_date, total
        FROM purchase_invoices
        WHERE id = $1 AND deleted_at IS NULL
        FOR SHARE
        "#,
        id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

// ---------------------------------------------------------------------------
// 銷項發票（#184）
// ---------------------------------------------------------------------------

/// 銷項發票帶入買受人資訊用的客戶資料（§8.11「買受人資訊」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomerForInvoice {
    pub id: Uuid,
    pub name: String,
    pub tax_id: Option<String>,
    pub address: Option<String>,
    pub invoice_title: Option<String>,
    pub invoice_address: Option<String>,
}

/// 未刪除的客戶 `id`。不存在或已刪除時為 `None`。
pub async fn find_customer_for_invoice<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
) -> Result<Option<CustomerForInvoice>, DbError> {
    let row = sqlx::query_as!(
        CustomerForInvoice,
        r#"
        SELECT id, name, tax_id, address, invoice_title, invoice_address
        FROM customers
        WHERE id = $1 AND deleted_at IS NULL
        "#,
        id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 發票（憑證）關聯的出貨單或進貨單（§8.11「關聯出貨單、進貨單」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentForInvoice {
    pub id: Uuid,
    pub record_no: String,
    /// `draft` / `posted` / `void`。
    pub status: String,
    /// 出貨單為客戶、進貨單為廠商。
    pub party_id: Uuid,
}

/// 未刪除的出貨單中 id 在 `ids` 內的（依 id 排序），以 `FOR SHARE` 鎖定（存檔時使用）。
pub async fn list_shipments_for_invoice<'e>(
    executor: impl PgExecutor<'e>,
    ids: &[Uuid],
) -> Result<Vec<DocumentForInvoice>, DbError> {
    let rows = sqlx::query_as!(
        DocumentForInvoice,
        r#"
        SELECT id, record_no, status, customer_id AS party_id
        FROM shipments
        WHERE id = ANY($1) AND deleted_at IS NULL
        ORDER BY id
        FOR SHARE
        "#,
        ids,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 同 [`list_shipments_for_invoice`]，但以 `FOR UPDATE` 依 id 順序鎖定（開立時使用；出貨單作廢時也以
/// `FOR UPDATE` 鎖定出貨單，所以兩者不會同時成功）。
pub async fn lock_shipments_for_invoice<'e>(
    executor: impl PgExecutor<'e>,
    ids: &[Uuid],
) -> Result<Vec<DocumentForInvoice>, DbError> {
    let rows = sqlx::query_as!(
        DocumentForInvoice,
        r#"
        SELECT id, record_no, status, customer_id AS party_id
        FROM shipments
        WHERE id = ANY($1) AND deleted_at IS NULL
        ORDER BY id
        FOR UPDATE
        "#,
        ids,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 出貨單已在某張有效（`posted`）銷項發票中。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvoicedDocument {
    /// 出貨單（進貨單）的 id。
    pub document_id: Uuid,
    pub invoice_id: Uuid,
    pub invoice_no: String,
    pub invoice_record_no: String,
}

/// `shipment_ids` 中已被有效銷項發票關聯的出貨單（依出貨單 id、發票 id 排序）。`exclude` 為不列入的發票。
pub async fn posted_invoices_of_shipments<'e>(
    executor: impl PgExecutor<'e>,
    shipment_ids: &[Uuid],
    exclude: Option<Uuid>,
) -> Result<Vec<InvoicedDocument>, DbError> {
    let rows = sqlx::query_as!(
        InvoicedDocument,
        r#"
        SELECT l.shipment_id AS document_id, i.id AS invoice_id, i.invoice_no,
               i.record_no AS invoice_record_no
        FROM sales_invoice_shipments l
        JOIN sales_invoices i ON i.id = l.sales_invoice_id
        WHERE l.shipment_id = ANY($1) AND l.deleted_at IS NULL
          AND i.status = 'posted' AND i.deleted_at IS NULL
          AND i.id IS DISTINCT FROM $2
        ORDER BY l.shipment_id, i.id
        "#,
        shipment_ids,
        exclude,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 發票關聯的出貨單 id（未刪除的關聯，依順序）。
pub async fn invoice_shipment_ids<'e>(
    executor: impl PgExecutor<'e>,
    invoice_id: Uuid,
) -> Result<Vec<Uuid>, DbError> {
    let ids = sqlx::query_scalar!(
        r#"
        SELECT shipment_id
        FROM sales_invoice_shipments
        WHERE sales_invoice_id = $1 AND deleted_at IS NULL
        ORDER BY sort_order, id
        "#,
        invoice_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(ids)
}

/// 發票明細的數量與單價（開立時重新檢查用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvoiceLineRow {
    pub id: Uuid,
    pub qty: Decimal,
    pub unit_price: Decimal,
}

/// 發票的未刪除明細（依順序）。
pub async fn list_sales_invoice_lines<'e>(
    executor: impl PgExecutor<'e>,
    invoice_id: Uuid,
) -> Result<Vec<InvoiceLineRow>, DbError> {
    let rows = sqlx::query_as!(
        InvoiceLineRow,
        r#"
        SELECT id, qty, unit_price
        FROM sales_invoice_lines
        WHERE sales_invoice_id = $1 AND deleted_at IS NULL
        ORDER BY sort_order, id
        "#,
        invoice_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 已使用發票號碼 `invoice_no` 的銷項發票（開立或已作廢，`exclude` 除外）：`(id, record_no)`。
pub async fn find_sales_invoice_by_no<'e>(
    executor: impl PgExecutor<'e>,
    invoice_no: &str,
    exclude: Uuid,
) -> Result<Option<(Uuid, String)>, DbError> {
    let row = sqlx::query!(
        r#"
        SELECT id, record_no
        FROM sales_invoices
        WHERE invoice_no = $1 AND status <> 'draft' AND id <> $2
        ORDER BY id
        LIMIT 1
        "#,
        invoice_no,
        exclude,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row.map(|r| (r.id, r.record_no)))
}

/// 指向發票的有效（`posted`）銷貨折讓：`(id, record_no)`，依 id 排序。
pub async fn posted_allowances_of_sales_invoice<'e>(
    executor: impl PgExecutor<'e>,
    invoice_id: Uuid,
) -> Result<Vec<(Uuid, String)>, DbError> {
    let rows = sqlx::query!(
        r#"
        SELECT id, record_no
        FROM sales_allowances
        WHERE sales_invoice_id = $1 AND status = 'posted' AND deleted_at IS NULL
        ORDER BY id
        "#,
        invoice_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows.into_iter().map(|r| (r.id, r.record_no)).collect())
}

/// 發票模組的單據主表（寫入作廢日期與原因用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvoiceTable {
    SalesInvoices,
    SalesAllowances,
    PurchaseInvoices,
    PurchaseAllowances,
}

impl InvoiceTable {
    fn name(self) -> &'static str {
        match self {
            Self::SalesInvoices => "sales_invoices",
            Self::SalesAllowances => "sales_allowances",
            Self::PurchaseInvoices => "purchase_invoices",
            Self::PurchaseAllowances => "purchase_allowances",
        }
    }
}

/// 寫入作廢日期與原因（作廢時、狀態變更之前，同一個 transaction 內；單據必須是已開立）。
pub async fn set_void_details<'e>(
    executor: impl PgExecutor<'e>,
    table: InvoiceTable,
    id: Uuid,
    void_date: Date,
    void_reason: &str,
) -> Result<(), DbError> {
    // 資料表名稱來自固定的列舉值，不是使用者輸入
    let sql = format!(
        "UPDATE {} SET void_date = $2, void_reason = $3 WHERE id = $1 AND status = 'posted'",
        table.name()
    );
    let result = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .bind(void_date)
        .bind(void_reason)
        .execute(executor)
        .await?;
    if result.rows_affected() != 1 {
        return Err(DbError::Conflict);
    }
    Ok(())
}

/// 指向客戶的未刪除銷項發票、銷貨折讓張數（不論狀態，§8.11「客戶、廠商被引用時的限制」）。
pub async fn count_customer_invoice_documents<'e>(
    executor: impl PgExecutor<'e>,
    customer_id: Uuid,
) -> Result<i64, DbError> {
    let count = sqlx::query_scalar!(
        r#"
        SELECT (SELECT count(*) FROM sales_invoices WHERE customer_id = $1 AND deleted_at IS NULL)
             + (SELECT count(*) FROM sales_allowances WHERE customer_id = $1 AND deleted_at IS NULL)
             AS "count!"
        "#,
        customer_id,
    )
    .fetch_one(executor)
    .await?;
    Ok(count)
}

/// 指向廠商的未刪除進項憑證、進貨折讓張數（不論狀態）。
pub async fn count_supplier_invoice_documents<'e>(
    executor: impl PgExecutor<'e>,
    supplier_id: Uuid,
) -> Result<i64, DbError> {
    let count = sqlx::query_scalar!(
        r#"
        SELECT (SELECT count(*) FROM purchase_invoices WHERE supplier_id = $1 AND deleted_at IS NULL)
             + (SELECT count(*) FROM purchase_allowances WHERE supplier_id = $1 AND deleted_at IS NULL)
             AS "count!"
        "#,
        supplier_id,
    )
    .fetch_one(executor)
    .await?;
    Ok(count)
}

// ---------------------------------------------------------------------------
// 銷貨折讓（#185）
// ---------------------------------------------------------------------------

/// 同 [`find_sales_invoice_for_allowance`]，但以 `FOR UPDATE` 鎖定（開立折讓時，同一張發票的折讓依序開立；
/// 作廢發票時框架也以 `FOR UPDATE` 鎖定發票）。
pub async fn lock_sales_invoice_for_allowance<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
) -> Result<Option<InvoiceForAllowance>, DbError> {
    let row = sqlx::query_as!(
        InvoiceForAllowance,
        r#"
        SELECT id, status, customer_id AS party_id, tax_type, invoice_type AS doc_type,
               invoice_date, total
        FROM sales_invoices
        WHERE id = $1 AND deleted_at IS NULL
        FOR UPDATE
        "#,
        id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 折讓關聯的退回單（銷貨退回、進貨退回）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReturnForAllowance {
    pub id: Uuid,
    pub record_no: String,
    /// `draft` / `posted` / `void`。
    pub status: String,
    /// 銷貨退回為客戶、進貨退回為廠商。
    pub party_id: Uuid,
}

/// 未刪除的銷貨退回；`lock` 為 `true` 時以 `FOR UPDATE` 鎖定（開立時），否則 `FOR SHARE`（存檔時）。
pub async fn find_sales_return_for_allowance<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    lock: bool,
) -> Result<Option<ReturnForAllowance>, DbError> {
    let row = if lock {
        sqlx::query_as!(
            ReturnForAllowance,
            r#"
            SELECT id, record_no, status, customer_id AS party_id
            FROM sales_returns
            WHERE id = $1 AND deleted_at IS NULL
            FOR UPDATE
            "#,
            id,
        )
        .fetch_optional(executor)
        .await?
    } else {
        sqlx::query_as!(
            ReturnForAllowance,
            r#"
            SELECT id, record_no, status, customer_id AS party_id
            FROM sales_returns
            WHERE id = $1 AND deleted_at IS NULL
            FOR SHARE
            "#,
            id,
        )
        .fetch_optional(executor)
        .await?
    };
    Ok(row)
}

/// 關聯銷貨退回的有效（`posted`）銷貨折讓：`(id, record_no)`，依 id 排序；`exclude` 除外。
pub async fn posted_allowances_of_sales_return<'e>(
    executor: impl PgExecutor<'e>,
    return_id: Uuid,
    exclude: Option<Uuid>,
) -> Result<Vec<(Uuid, String)>, DbError> {
    let rows = sqlx::query!(
        r#"
        SELECT id, record_no
        FROM sales_allowances
        WHERE sales_return_id = $1 AND status = 'posted' AND deleted_at IS NULL
          AND id IS DISTINCT FROM $2
        ORDER BY id
        "#,
        return_id,
        exclude,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows.into_iter().map(|r| (r.id, r.record_no)).collect())
}

/// 發票的有效（`posted`）銷貨折讓 `total` 合計（`exclude` 除外）。
pub async fn posted_allowance_total_of_sales_invoice<'e>(
    executor: impl PgExecutor<'e>,
    invoice_id: Uuid,
    exclude: Option<Uuid>,
) -> Result<Decimal, DbError> {
    let total = sqlx::query_scalar!(
        r#"
        SELECT COALESCE(sum(total), 0) AS "total!"
        FROM sales_allowances
        WHERE sales_invoice_id = $1 AND status = 'posted' AND deleted_at IS NULL
          AND id IS DISTINCT FROM $2
        "#,
        invoice_id,
        exclude,
    )
    .fetch_one(executor)
    .await?;
    Ok(total)
}

/// 銷貨折讓的未刪除明細（依順序）。
pub async fn list_sales_allowance_lines<'e>(
    executor: impl PgExecutor<'e>,
    allowance_id: Uuid,
) -> Result<Vec<InvoiceLineRow>, DbError> {
    let rows = sqlx::query_as!(
        InvoiceLineRow,
        r#"
        SELECT id, qty, unit_price
        FROM sales_allowance_lines
        WHERE sales_allowance_id = $1 AND deleted_at IS NULL
        ORDER BY sort_order, id
        "#,
        allowance_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// 進項憑證、進貨折讓（#186）
// ---------------------------------------------------------------------------

/// 未刪除廠商的統一編號（外層 `None` 為廠商不存在或已刪除）。
pub async fn find_supplier_tax_id<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
) -> Result<Option<Option<String>>, DbError> {
    let row = sqlx::query_scalar!(
        "SELECT tax_id FROM suppliers WHERE id = $1 AND deleted_at IS NULL",
        id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 未刪除的進貨單中 id 在 `ids` 內的（依 id 排序）；`lock` 為開立時以 `FOR UPDATE` 鎖定，否則 `FOR SHARE`。
pub async fn list_receipts_for_invoice<'e>(
    executor: impl PgExecutor<'e>,
    ids: &[Uuid],
    lock: bool,
) -> Result<Vec<DocumentForInvoice>, DbError> {
    let rows = if lock {
        sqlx::query_as!(
            DocumentForInvoice,
            r#"
            SELECT id, record_no, status, supplier_id AS party_id
            FROM goods_receipts
            WHERE id = ANY($1) AND deleted_at IS NULL
            ORDER BY id
            FOR UPDATE
            "#,
            ids,
        )
        .fetch_all(executor)
        .await?
    } else {
        sqlx::query_as!(
            DocumentForInvoice,
            r#"
            SELECT id, record_no, status, supplier_id AS party_id
            FROM goods_receipts
            WHERE id = ANY($1) AND deleted_at IS NULL
            ORDER BY id
            FOR SHARE
            "#,
            ids,
        )
        .fetch_all(executor)
        .await?
    };
    Ok(rows)
}

/// `receipt_ids` 中已被有效（`posted`）進項憑證關聯的進貨單。`customs` 為海關繳納證，否則為發票類
/// （三聯式、電子發票）；兩類各自計算（§8.11）。`exclude` 為不列入的憑證。
pub async fn posted_invoices_of_receipts<'e>(
    executor: impl PgExecutor<'e>,
    receipt_ids: &[Uuid],
    customs: Option<bool>,
    exclude: Option<Uuid>,
) -> Result<Vec<InvoicedDocument>, DbError> {
    let rows = sqlx::query_as!(
        InvoicedDocument,
        r#"
        SELECT l.goods_receipt_id AS document_id, i.id AS invoice_id, i.invoice_no,
               i.record_no AS invoice_record_no
        FROM purchase_invoice_receipts l
        JOIN purchase_invoices i ON i.id = l.purchase_invoice_id
        WHERE l.goods_receipt_id = ANY($1) AND l.deleted_at IS NULL
          AND i.status = 'posted' AND i.deleted_at IS NULL
          AND ($2::boolean IS NULL OR (i.doc_type = 'customs') = $2)
          AND i.id IS DISTINCT FROM $3
        ORDER BY l.goods_receipt_id, i.id
        "#,
        receipt_ids,
        customs,
        exclude,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 進項憑證關聯的進貨單 id（未刪除的關聯，依順序）。
pub async fn invoice_receipt_ids<'e>(
    executor: impl PgExecutor<'e>,
    invoice_id: Uuid,
) -> Result<Vec<Uuid>, DbError> {
    let ids = sqlx::query_scalar!(
        r#"
        SELECT goods_receipt_id
        FROM purchase_invoice_receipts
        WHERE purchase_invoice_id = $1 AND deleted_at IS NULL
        ORDER BY sort_order, id
        "#,
        invoice_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(ids)
}

/// 同一廠商已登錄（`posted`）號碼 `invoice_no` 的進項憑證（`exclude` 除外）：`(id, record_no)`。
pub async fn find_purchase_invoice_by_no<'e>(
    executor: impl PgExecutor<'e>,
    supplier_id: Uuid,
    invoice_no: &str,
    exclude: Uuid,
) -> Result<Option<(Uuid, String)>, DbError> {
    let row = sqlx::query!(
        r#"
        SELECT id, record_no
        FROM purchase_invoices
        WHERE supplier_id = $1 AND invoice_no = $2 AND status = 'posted' AND id <> $3
        ORDER BY id
        LIMIT 1
        "#,
        supplier_id,
        invoice_no,
        exclude,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row.map(|r| (r.id, r.record_no)))
}

/// 指向進項憑證的有效（`posted`）進貨折讓：`(id, record_no)`，依 id 排序。
pub async fn posted_allowances_of_purchase_invoice<'e>(
    executor: impl PgExecutor<'e>,
    invoice_id: Uuid,
) -> Result<Vec<(Uuid, String)>, DbError> {
    let rows = sqlx::query!(
        r#"
        SELECT id, record_no
        FROM purchase_allowances
        WHERE purchase_invoice_id = $1 AND status = 'posted' AND deleted_at IS NULL
        ORDER BY id
        "#,
        invoice_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows.into_iter().map(|r| (r.id, r.record_no)).collect())
}

/// 同 [`find_purchase_invoice_for_allowance`]，但以 `FOR UPDATE` 鎖定（開立進貨折讓時）。
pub async fn lock_purchase_invoice_for_allowance<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
) -> Result<Option<InvoiceForAllowance>, DbError> {
    let row = sqlx::query_as!(
        InvoiceForAllowance,
        r#"
        SELECT id, status, supplier_id AS party_id, tax_type, doc_type, invoice_date, total
        FROM purchase_invoices
        WHERE id = $1 AND deleted_at IS NULL
        FOR UPDATE
        "#,
        id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 未刪除的進貨退回；`lock` 為 `true` 時以 `FOR UPDATE` 鎖定，否則 `FOR SHARE`。
pub async fn find_purchase_return_for_allowance<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    lock: bool,
) -> Result<Option<ReturnForAllowance>, DbError> {
    let row = if lock {
        sqlx::query_as!(
            ReturnForAllowance,
            r#"
            SELECT id, record_no, status, supplier_id AS party_id
            FROM purchase_returns
            WHERE id = $1 AND deleted_at IS NULL
            FOR UPDATE
            "#,
            id,
        )
        .fetch_optional(executor)
        .await?
    } else {
        sqlx::query_as!(
            ReturnForAllowance,
            r#"
            SELECT id, record_no, status, supplier_id AS party_id
            FROM purchase_returns
            WHERE id = $1 AND deleted_at IS NULL
            FOR SHARE
            "#,
            id,
        )
        .fetch_optional(executor)
        .await?
    };
    Ok(row)
}

/// 關聯進貨退回的有效（`posted`）進貨折讓：`(id, record_no)`，依 id 排序；`exclude` 除外。
pub async fn posted_allowances_of_purchase_return<'e>(
    executor: impl PgExecutor<'e>,
    return_id: Uuid,
    exclude: Option<Uuid>,
) -> Result<Vec<(Uuid, String)>, DbError> {
    let rows = sqlx::query!(
        r#"
        SELECT id, record_no
        FROM purchase_allowances
        WHERE purchase_return_id = $1 AND status = 'posted' AND deleted_at IS NULL
          AND id IS DISTINCT FROM $2
        ORDER BY id
        "#,
        return_id,
        exclude,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows.into_iter().map(|r| (r.id, r.record_no)).collect())
}

/// 進項憑證的有效（`posted`）進貨折讓 `total` 合計（`exclude` 除外）。
pub async fn posted_allowance_total_of_purchase_invoice<'e>(
    executor: impl PgExecutor<'e>,
    invoice_id: Uuid,
    exclude: Option<Uuid>,
) -> Result<Decimal, DbError> {
    let total = sqlx::query_scalar!(
        r#"
        SELECT COALESCE(sum(total), 0) AS "total!"
        FROM purchase_allowances
        WHERE purchase_invoice_id = $1 AND status = 'posted' AND deleted_at IS NULL
          AND id IS DISTINCT FROM $2
        "#,
        invoice_id,
        exclude,
    )
    .fetch_one(executor)
    .await?;
    Ok(total)
}

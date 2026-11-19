-- 0015_invoice_void_check：放寬發票模組作廢日期的 CHECK（#184，docs/architecture.md §8.11）。
--
-- 0014 的 CHECK ((status = 'void') = (void_date IS NOT NULL)) 要求作廢日期與「已作廢」同時成立，但作廢時
-- 狀態由單據框架（hammer_core::documents）在處理函式之後才變更，PostgreSQL 的 CHECK 又不能延後到 commit
-- 才檢查，所以兩個寫入無法分開進行。
--
-- 改為：
-- - 草稿沒有作廢日期：CHECK (status <> 'draft' OR void_date IS NULL)
-- - 已作廢一定有作廢日期：CHECK (status <> 'void' OR void_date IS NOT NULL)
--
-- 作廢的處理函式先寫入作廢日期與原因，框架再把狀態改為 void；兩者在同一個 transaction 內，作廢失敗時
-- 一起 rollback，所以 commit 後「已開立但有作廢日期」的資料不會出現。

ALTER TABLE sales_invoices DROP CONSTRAINT sales_invoices_void_date_check;
ALTER TABLE sales_invoices ADD CONSTRAINT sales_invoices_void_date_check
    CHECK (status <> 'draft' OR void_date IS NULL);
ALTER TABLE sales_invoices ADD CONSTRAINT sales_invoices_void_date_required_check
    CHECK (status <> 'void' OR void_date IS NOT NULL);

ALTER TABLE sales_allowances DROP CONSTRAINT sales_allowances_void_date_check;
ALTER TABLE sales_allowances ADD CONSTRAINT sales_allowances_void_date_check
    CHECK (status <> 'draft' OR void_date IS NULL);
ALTER TABLE sales_allowances ADD CONSTRAINT sales_allowances_void_date_required_check
    CHECK (status <> 'void' OR void_date IS NOT NULL);

ALTER TABLE purchase_invoices DROP CONSTRAINT purchase_invoices_void_date_check;
ALTER TABLE purchase_invoices ADD CONSTRAINT purchase_invoices_void_date_check
    CHECK (status <> 'draft' OR void_date IS NULL);
ALTER TABLE purchase_invoices ADD CONSTRAINT purchase_invoices_void_date_required_check
    CHECK (status <> 'void' OR void_date IS NOT NULL);

ALTER TABLE purchase_allowances DROP CONSTRAINT purchase_allowances_void_date_check;
ALTER TABLE purchase_allowances ADD CONSTRAINT purchase_allowances_void_date_check
    CHECK (status <> 'draft' OR void_date IS NULL);
ALTER TABLE purchase_allowances ADD CONSTRAINT purchase_allowances_void_date_required_check
    CHECK (status <> 'void' OR void_date IS NOT NULL);

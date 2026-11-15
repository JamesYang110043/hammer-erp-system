-- 刪除自訂資料類別（#73，docs/architecture.md §7.1、§8.5）。
--
-- 資料類別以軟刪除（entity_definitions.deleted_at）刪除，schema_change_logs 記為 delete。
-- 資料（custom_records）保留；代碼的 UNIQUE 限制包含已刪除的資料類別，刪除後代碼不可重複使用。

ALTER TABLE schema_change_logs DROP CONSTRAINT schema_change_logs_action_check;
ALTER TABLE schema_change_logs ADD CONSTRAINT schema_change_logs_action_check
    CHECK (action IN ('create', 'update', 'deactivate', 'delete'));

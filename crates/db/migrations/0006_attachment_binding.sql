-- 附件的上傳與綁定（#71，docs/architecture.md §7.3、§8.4 attachments）。
--
-- 附件先上傳（未綁定），之後在新增或修改資料時寫入 file 欄位才綁定到該筆資料的欄位。
-- 未綁定時 entity_key、record_id、field_key 都是 NULL；綁定後三者都有值。
-- 未綁定超過時限（24 小時）的附件由清理函式刪除資料列與檔案。

ALTER TABLE attachments
    ALTER COLUMN entity_key DROP NOT NULL,
    ALTER COLUMN record_id DROP NOT NULL,
    ALTER COLUMN field_key DROP NOT NULL;

ALTER TABLE attachments
    ADD CONSTRAINT ck_attachments_binding CHECK (
        (entity_key IS NULL AND record_id IS NULL AND field_key IS NULL)
        OR (entity_key IS NOT NULL AND record_id IS NOT NULL AND field_key IS NOT NULL)
    );

COMMENT ON COLUMN attachments.entity_key IS '附件所屬資料的資料類別代碼；未綁定時為 NULL';
COMMENT ON COLUMN attachments.record_id IS '附件所屬的資料；可能是系統資料表或 custom_records，因此不設外鍵。未綁定時為 NULL';
COMMENT ON COLUMN attachments.field_key IS '附件所屬的 file 欄位；未綁定時為 NULL';
COMMENT ON COLUMN attachments.storage_path IS '相對於附件目錄的路徑，以附件 id 命名（原始檔名只存在 file_name）';
COMMENT ON COLUMN attachments.deleted_at IS '欄位值被換掉或清空時標記；檔案保留';

-- 清理未綁定的附件
CREATE INDEX idx_attachments_unbound ON attachments (created_at) WHERE record_id IS NULL;

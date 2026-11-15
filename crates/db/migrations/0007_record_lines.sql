-- 明細子表的資料列（#72，docs/architecture.md §8.5 custom_records、§10）。
--
-- 明細（kind = detail 的資料類別）的資料也存在 custom_records，parent_record_id 指向主表的資料。
-- 明細的順序依 sort_order（存檔時 request 陣列中的位置，從 0 開始），同值時依 id。
-- 主表與一般資料的 sort_order 固定為 0。

ALTER TABLE custom_records ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0;

COMMENT ON COLUMN custom_records.sort_order IS '明細的順序（從 0 開始）；主表的資料固定為 0';

-- 依主表讀取明細並排序；取代原本只有 parent_record_id 的索引
DROP INDEX idx_custom_records_parent_record_id;
CREATE INDEX idx_custom_records_parent_record_id
    ON custom_records (parent_record_id, entity_id, sort_order, id)
    WHERE parent_record_id IS NOT NULL;

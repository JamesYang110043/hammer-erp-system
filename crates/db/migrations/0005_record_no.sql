-- 自訂資料類別的單號（#68，docs/architecture.md §8.4 number_sequences、§8.5 custom_records）。
--
-- 同一個資料類別的單號不可重複。已軟刪除的資料也計入：刪除後單號不重複使用。
-- 沒有單號（資料類別沒有設定單號規則）的資料不受限制。
-- 此索引也用於列表依 record_no 排序與篩選。

CREATE UNIQUE INDEX uq_custom_records_entity_record_no
    ON custom_records (entity_id, record_no)
    WHERE record_no IS NOT NULL;

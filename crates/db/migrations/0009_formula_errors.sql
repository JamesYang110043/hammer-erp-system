-- 存檔時計算公式（#91，docs/architecture.md §7.6、§8.5 `custom_records`）。
--
-- 公式的結果寫入 data；計算失敗的欄位在 data 中沒有值，失敗原因記在 formula_errors：
-- 鍵為欄位代碼、值為錯誤原因（繁體中文）。沒有任何錯誤時為 NULL。

ALTER TABLE custom_records ADD COLUMN formula_errors JSONB;
ALTER TABLE custom_records ADD CONSTRAINT custom_records_formula_errors_object
    CHECK (formula_errors IS NULL OR jsonb_typeof(formula_errors) = 'object');

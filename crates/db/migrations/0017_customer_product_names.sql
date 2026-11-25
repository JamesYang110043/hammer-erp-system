-- 0017_customer_product_names：M11 客戶品名（#264，docs/architecture.md §8.7、§8.9「明細的品名」）。
--
-- 1. customer_product_names（明細子表 customer_product_name，主表 customer）：客戶 × 商品 → 銷貨品名。
--    依 §8.6「系統資料表的共同規則」；同一客戶同一商品最多一筆未刪除的資料（部分唯一索引）。
-- 2. sales_order_lines、shipment_lines、sales_return_lines 新增 product_name（單據上的品名快照）：
--    先加可為 NULL 的欄位 → 以商品的 name 補上既有資料（銷貨退回明細取出貨明細補好的值）→ 再設 NOT NULL。
--    ALTER TABLE 只能把欄位加在最後，所以實體欄位的位置在共通欄位之後；欄位定義的 sort_order
--    插在 product_id 之後（訂單明細 15、出貨明細與退回明細 25）。
--
-- 帶入、檢查等業務邏輯在 hammer_core（#265），此處只建立資料結構。
-- 資料類別、欄位定義的 id 為固定的 uuid v7；欄位定義必須與 architecture.md 一致，
-- 由 crates/db/tests/customer_product_names.rs、crates/db/tests/sales.rs 檢查。

-- ============================================================================
-- customer_product_names（明細子表 customer_product_name）
-- ============================================================================

CREATE TABLE customer_product_names (
    id              UUID PRIMARY KEY,
    customer_id     UUID NOT NULL REFERENCES customers(id),
    sort_order      INTEGER NOT NULL DEFAULT 0,
    product_id      UUID NOT NULL REFERENCES products(id),
    name            TEXT NOT NULL,
    notes           TEXT,
    custom_data     JSONB NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(custom_data) = 'object'),
    formula_errors  JSONB CHECK (formula_errors IS NULL OR jsonb_typeof(formula_errors) = 'object'),
    -- 共通欄位（§8.1）
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by      UUID REFERENCES users(id),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by      UUID REFERENCES users(id),
    deleted_at      TIMESTAMPTZ,
    version         INTEGER NOT NULL DEFAULT 1
);

COMMENT ON TABLE customer_product_names IS '客戶品名（§8.7，明細子表 customer_product_name）：同一商品依客戶使用不同的銷貨品名';
COMMENT ON COLUMN customer_product_names.customer_id IS '所屬客戶（不是欄位）';
COMMENT ON COLUMN customer_product_names.sort_order IS '明細順序，從 0 開始（不是欄位）';
COMMENT ON COLUMN customer_product_names.name IS '客戶品名；銷貨單據明細的品名依此帶入（§8.9「明細的品名」）';

CREATE INDEX idx_customer_product_names_customer_id
    ON customer_product_names (customer_id, sort_order, id) WHERE deleted_at IS NULL;
CREATE INDEX idx_customer_product_names_product_id ON customer_product_names (product_id);
CREATE INDEX idx_customer_product_names_name ON customer_product_names (lower(name));
CREATE INDEX idx_customer_product_names_custom_data ON customer_product_names USING GIN (custom_data);
CREATE UNIQUE INDEX uq_customer_product_names_customer_product
    ON customer_product_names (customer_id, product_id) WHERE deleted_at IS NULL;

CREATE TRIGGER trg_customer_product_names_set_updated_at
    BEFORE UPDATE ON customer_product_names
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

INSERT INTO entity_definitions
    (id, key, label, kind, is_system, storage, table_name, parent_entity_id, title_template, sort_order)
VALUES
    ('01a211f5-20e9-7b08-9d17-2a0dbce98c12', 'customer_product_name', '客戶品名', 'detail', true, 'table',
     'customer_product_names', '01a1e516-3404-7153-bc28-b536c04db4db', '{product_id.code} {name}', 10);

INSERT INTO field_definitions
    (id, entity_id, field_key, label, data_type, is_system, storage, column_name,
     is_required, is_searchable, is_unique, default_value, validation, options,
     decimal_places, relation_entity_id, ui_widget, sort_order)
VALUES
    ('01a211f5-20ea-704c-9ef5-4cb1c369cd4d', '01a211f5-20e9-7b08-9d17-2a0dbce98c12', 'product_id', '商品', 'relation', true, 'column', 'product_id',
     true, false, false, NULL, NULL, NULL,
     NULL, '01a1e516-3401-7432-acb3-9cc3ac6a7db9', NULL, 10),
    ('01a211f5-20eb-7625-adc6-b1bfebb5e6d9', '01a211f5-20e9-7b08-9d17-2a0dbce98c12', 'name', '客戶品名', 'text', true, 'column', 'name',
     true, true, false, NULL, '{"max_length":200}'::jsonb, NULL,
     NULL, NULL, NULL, 20),
    ('01a211f5-20ec-7c63-9806-17455d5c8cd3', '01a211f5-20e9-7b08-9d17-2a0dbce98c12', 'notes', '備註', 'long_text', true, 'column', 'notes',
     false, false, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 30);

-- ============================================================================
-- 銷貨明細的品名（product_name）
-- ============================================================================

ALTER TABLE sales_order_lines ADD COLUMN product_name TEXT;
ALTER TABLE shipment_lines ADD COLUMN product_name TEXT;
ALTER TABLE sales_return_lines ADD COLUMN product_name TEXT;

-- 既有資料：訂單明細、出貨明細以當時商品的 name 補上（含已刪除的商品與明細）
UPDATE sales_order_lines l SET product_name = p.name FROM products p WHERE p.id = l.product_id;
UPDATE shipment_lines l SET product_name = p.name FROM products p WHERE p.id = l.product_id;
-- 銷貨退回明細 = 出貨明細的品名
UPDATE sales_return_lines r SET product_name = s.product_name
    FROM shipment_lines s WHERE s.id = r.shipment_line_id;

ALTER TABLE sales_order_lines ALTER COLUMN product_name SET NOT NULL;
ALTER TABLE shipment_lines ALTER COLUMN product_name SET NOT NULL;
ALTER TABLE sales_return_lines ALTER COLUMN product_name SET NOT NULL;

COMMENT ON COLUMN sales_order_lines.product_name IS '品名：單據上的品名快照，沒帶時依客戶品名 → 商品名稱帶入（§8.9）';
COMMENT ON COLUMN shipment_lines.product_name IS '品名：單據上的品名快照；有訂單明細時帶入訂單明細的品名（§8.9）';
COMMENT ON COLUMN sales_return_lines.product_name IS '品名 = 出貨明細的品名，系統維護（§8.9）';

INSERT INTO field_definitions
    (id, entity_id, field_key, label, data_type, is_system, storage, column_name,
     is_required, is_visible, is_readonly, default_value, validation, options,
     decimal_places, relation_entity_id, ui_widget, sort_order)
VALUES
    -- sales_order_line
    ('01a211f5-20ed-7ff2-a9e5-fe8137490b45', '01a1f9d2-7b30-7001-810e-1c4e6e7ccaee', 'product_name', '品名', 'text', true, 'column', 'product_name',
     true, true, false, NULL, '{"max_length":200}'::jsonb, NULL,
     NULL, NULL, NULL, 15),
    -- shipment_line
    ('01a211f5-20ee-7895-a677-9db34d28ad28', '01a1f9d2-7b30-7003-a04b-d9224e8eaf33', 'product_name', '品名', 'text', true, 'column', 'product_name',
     true, true, false, NULL, '{"max_length":200}'::jsonb, NULL,
     NULL, NULL, NULL, 25),
    -- sales_return_line（系統維護）
    ('01a211f5-20ef-7966-b3dc-a8e434dd1f53', '01a1f9d2-7b30-7005-96eb-7d816fcd7344', 'product_name', '品名', 'text', true, 'column', 'product_name',
     false, true, true, NULL, NULL, NULL,
     NULL, NULL, NULL, 25);

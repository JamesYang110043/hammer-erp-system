-- 0013_sales：M4c 訂單、出貨單、銷貨退回的資料表與資料類別（#146，docs/architecture.md §8.9）。
--
-- 建立三張系統單據與其明細子表，依 §8.6「系統資料表的共同規則」與 §7.1「系統單據」：
-- 系統欄位 → custom_data → formula_errors → 共通欄位；單號 record_no 與確認（過帳）、作廢、結案的時間與人員、
-- 出貨明細的 lot_id 不是欄位。銷貨以台幣交易，沒有幣別與匯率。
--
-- 出貨的手動指定批號存在出貨明細的 lot_no（一列一個批號，留空 = 先進先出），不另建草稿資料表（§8.9「出貨的批號」）。
-- 銷貨退回加回的批號記在 inventory_transactions（txn_type = sales_return），也不另建資料表。
--
-- 系統維護的欄位（status、金額合計、amount、shipped_qty、成本等）是唯讀的系統欄位；
-- 金額欄位以 DEFAULT 0 建立，由 hammer_core 在存檔時計算（#147）。
--
-- 庫存的 lot_allocations.source_type、inventory_transactions.ref_doc_type 加上出貨單與銷貨退回的代碼（§8.10）。
--
-- 資料類別、欄位定義的 id 為固定的 uuid v7，方便跨環境比對。
-- 欄位定義必須與 architecture.md §8.9 一致，由 crates/db/tests/sales.rs 檢查。

-- ============================================================================
-- sales_orders（訂單，系統單據 sales_order）
-- ============================================================================

CREATE TABLE sales_orders (
    id               UUID PRIMARY KEY,
    record_no        TEXT NOT NULL,
    customer_id      UUID NOT NULL REFERENCES customers(id),
    order_date       DATE NOT NULL,
    delivery_date    DATE,
    tax_type         TEXT NOT NULL REFERENCES tax_rates(code),
    tax_included     BOOLEAN NOT NULL DEFAULT false,
    status           TEXT NOT NULL DEFAULT 'draft'
                     CHECK (status IN ('draft', 'confirmed', 'partially_shipped', 'closed', 'void')),
    subtotal         NUMERIC(18,4) NOT NULL DEFAULT 0,
    tax_amount       NUMERIC(18,4) NOT NULL DEFAULT 0,
    total            NUMERIC(18,4) NOT NULL DEFAULT 0,
    notes            TEXT,
    posted_at        TIMESTAMPTZ,
    posted_by        UUID REFERENCES users(id),
    voided_at        TIMESTAMPTZ,
    voided_by        UUID REFERENCES users(id),
    closed_at        TIMESTAMPTZ,
    closed_by        UUID REFERENCES users(id),
    custom_data      JSONB NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(custom_data) = 'object'),
    formula_errors   JSONB CHECK (formula_errors IS NULL OR jsonb_typeof(formula_errors) = 'object'),
    -- 共通欄位（§8.1）
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by       UUID REFERENCES users(id),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by       UUID REFERENCES users(id),
    deleted_at       TIMESTAMPTZ,
    version          INTEGER NOT NULL DEFAULT 1,

    CONSTRAINT sales_orders_posted_status_check
        CHECK ((status = 'draft') = (posted_at IS NULL)),
    CONSTRAINT sales_orders_voided_status_check
        CHECK ((status = 'void') = (voided_at IS NOT NULL)),
    CONSTRAINT sales_orders_posted_by_check
        CHECK ((posted_at IS NULL) = (posted_by IS NULL)),
    CONSTRAINT sales_orders_voided_by_check
        CHECK ((voided_at IS NULL) = (voided_by IS NULL)),
    CONSTRAINT sales_orders_closed_by_check
        CHECK ((closed_at IS NULL) = (closed_by IS NULL)),
    CONSTRAINT sales_orders_closed_status_check
        CHECK (closed_at IS NULL OR status = 'closed')
);

COMMENT ON TABLE sales_orders IS '訂單（§8.9，系統單據 sales_order）';
COMMENT ON COLUMN sales_orders.record_no IS '單號（不是欄位），依 numbering_pattern 取號；唯一索引含已刪除的資料';
COMMENT ON COLUMN sales_orders.tax_included IS '單價是否含稅；明細的 amount 與單價同一個基準（§8.9「金額與稅額」）';
COMMENT ON COLUMN sales_orders.status IS 'draft / confirmed / partially_shipped / closed / void，系統維護（§8.9「訂單的狀態」）';
COMMENT ON COLUMN sales_orders.subtotal IS '未稅金額（台幣），系統維護';
COMMENT ON COLUMN sales_orders.posted_at IS '確認時間（不是欄位）';
COMMENT ON COLUMN sales_orders.voided_at IS '作廢時間（不是欄位）';
COMMENT ON COLUMN sales_orders.closed_at IS '手動結案時間（不是欄位）；自動結案時為 NULL';

CREATE UNIQUE INDEX uq_sales_orders_record_no ON sales_orders (record_no);
CREATE INDEX idx_sales_orders_order_date ON sales_orders (order_date, id);
CREATE INDEX idx_sales_orders_status ON sales_orders (status);
CREATE INDEX idx_sales_orders_customer_id ON sales_orders (customer_id);
CREATE INDEX idx_sales_orders_custom_data ON sales_orders USING GIN (custom_data);

CREATE TRIGGER trg_sales_orders_set_updated_at
    BEFORE UPDATE ON sales_orders
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- sales_order_lines（明細子表 sales_order_line）
-- ============================================================================

CREATE TABLE sales_order_lines (
    id                 UUID PRIMARY KEY,
    sales_order_id     UUID NOT NULL REFERENCES sales_orders(id),
    sort_order         INTEGER NOT NULL DEFAULT 0,
    product_id         UUID NOT NULL REFERENCES products(id),
    qty                NUMERIC(18,4) NOT NULL CHECK (qty > 0),
    unit_price         NUMERIC(18,4) NOT NULL CHECK (unit_price >= 0),
    discount_rate      NUMERIC(6,4) NOT NULL DEFAULT 0 CHECK (discount_rate >= 0 AND discount_rate <= 1),
    amount             NUMERIC(18,4) NOT NULL DEFAULT 0,
    shipped_qty        NUMERIC(18,4) NOT NULL DEFAULT 0 CHECK (shipped_qty >= 0),
    notes              TEXT,
    custom_data        JSONB NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(custom_data) = 'object'),
    formula_errors     JSONB CHECK (formula_errors IS NULL OR jsonb_typeof(formula_errors) = 'object'),
    -- 共通欄位（§8.1）
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by         UUID REFERENCES users(id),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by         UUID REFERENCES users(id),
    deleted_at         TIMESTAMPTZ,
    version            INTEGER NOT NULL DEFAULT 1
);

COMMENT ON TABLE sales_order_lines IS '訂單明細（§8.9，明細子表 sales_order_line）';
COMMENT ON COLUMN sales_order_lines.sales_order_id IS '所屬單據（不是欄位）';
COMMENT ON COLUMN sales_order_lines.sort_order IS '明細順序，從 0 開始（不是欄位）';
COMMENT ON COLUMN sales_order_lines.unit_price IS '單價；含稅或未稅依表頭的 tax_included';
COMMENT ON COLUMN sales_order_lines.discount_rate IS '折扣率：減少的比例，0.1 = 打九折';
COMMENT ON COLUMN sales_order_lines.amount IS 'round(qty × unit_price × (1 − discount_rate), 台幣小數位數)，系統維護';
COMMENT ON COLUMN sales_order_lines.shipped_qty IS '已出貨數量：出貨單過帳時增加、作廢時減少，系統維護';

CREATE INDEX idx_sales_order_lines_sales_order_id
    ON sales_order_lines (sales_order_id, sort_order, id) WHERE deleted_at IS NULL;
CREATE INDEX idx_sales_order_lines_product_id ON sales_order_lines (product_id);
CREATE INDEX idx_sales_order_lines_custom_data ON sales_order_lines USING GIN (custom_data);

CREATE TRIGGER trg_sales_order_lines_set_updated_at
    BEFORE UPDATE ON sales_order_lines
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- shipments（出貨單，系統單據 shipment）
-- ============================================================================

CREATE TABLE shipments (
    id                 UUID PRIMARY KEY,
    record_no          TEXT NOT NULL,
    customer_id        UUID NOT NULL REFERENCES customers(id),
    sales_order_id     UUID REFERENCES sales_orders(id),
    ship_date          DATE NOT NULL,
    tax_type           TEXT NOT NULL REFERENCES tax_rates(code),
    tax_included       BOOLEAN NOT NULL DEFAULT false,
    status             TEXT NOT NULL DEFAULT 'draft'
                       CHECK (status IN ('draft', 'posted', 'void')),
    subtotal           NUMERIC(18,4) NOT NULL DEFAULT 0,
    tax_amount         NUMERIC(18,4) NOT NULL DEFAULT 0,
    total              NUMERIC(18,4) NOT NULL DEFAULT 0,
    cost_total_twd     NUMERIC(18,2),
    ship_address       TEXT,
    notes              TEXT,
    posted_at          TIMESTAMPTZ,
    posted_by          UUID REFERENCES users(id),
    voided_at          TIMESTAMPTZ,
    voided_by          UUID REFERENCES users(id),
    custom_data        JSONB NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(custom_data) = 'object'),
    formula_errors     JSONB CHECK (formula_errors IS NULL OR jsonb_typeof(formula_errors) = 'object'),
    -- 共通欄位（§8.1）
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by         UUID REFERENCES users(id),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by         UUID REFERENCES users(id),
    deleted_at         TIMESTAMPTZ,
    version            INTEGER NOT NULL DEFAULT 1,

    CONSTRAINT shipments_posted_status_check
        CHECK ((status = 'draft') = (posted_at IS NULL)),
    CONSTRAINT shipments_voided_status_check
        CHECK ((status = 'void') = (voided_at IS NOT NULL)),
    CONSTRAINT shipments_posted_by_check
        CHECK ((posted_at IS NULL) = (posted_by IS NULL)),
    CONSTRAINT shipments_voided_by_check
        CHECK ((voided_at IS NULL) = (voided_by IS NULL))
);

COMMENT ON TABLE shipments IS '出貨單（§8.9，系統單據 shipment）';
COMMENT ON COLUMN shipments.record_no IS '單號（不是欄位），依 numbering_pattern 取號；唯一索引含已刪除的資料';
COMMENT ON COLUMN shipments.sales_order_id IS '來源訂單；可以不經訂單直接出貨';
COMMENT ON COLUMN shipments.ship_date IS '出貨日期：異動的 txn_date';
COMMENT ON COLUMN shipments.status IS 'draft / posted / void，系統維護（§7.1、§8.2）';
COMMENT ON COLUMN shipments.cost_total_twd IS '銷貨成本：明細 cost_amount_twd 的合計，過帳時寫入；草稿為 NULL';
COMMENT ON COLUMN shipments.posted_at IS '過帳時間（不是欄位）';
COMMENT ON COLUMN shipments.voided_at IS '作廢時間（不是欄位）';

CREATE UNIQUE INDEX uq_shipments_record_no ON shipments (record_no);
CREATE INDEX idx_shipments_ship_date ON shipments (ship_date, id);
CREATE INDEX idx_shipments_status ON shipments (status);
CREATE INDEX idx_shipments_customer_id ON shipments (customer_id);
CREATE INDEX idx_shipments_sales_order_id ON shipments (sales_order_id);
CREATE INDEX idx_shipments_custom_data ON shipments USING GIN (custom_data);

CREATE TRIGGER trg_shipments_set_updated_at
    BEFORE UPDATE ON shipments
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- shipment_lines（明細子表 shipment_line）
-- ============================================================================

CREATE TABLE shipment_lines (
    id                   UUID PRIMARY KEY,
    shipment_id          UUID NOT NULL REFERENCES shipments(id),
    sort_order           INTEGER NOT NULL DEFAULT 0,
    sales_order_line_id  UUID REFERENCES sales_order_lines(id),
    product_id           UUID NOT NULL REFERENCES products(id),
    qty                  NUMERIC(18,4) NOT NULL CHECK (qty > 0),
    unit_price           NUMERIC(18,4) NOT NULL CHECK (unit_price >= 0),
    discount_rate        NUMERIC(6,4) NOT NULL DEFAULT 0 CHECK (discount_rate >= 0 AND discount_rate <= 1),
    amount               NUMERIC(18,4) NOT NULL DEFAULT 0,
    lot_no               TEXT,
    cost_amount_twd      NUMERIC(18,2),
    notes                TEXT,
    lot_id               UUID REFERENCES inventory_lots(id),
    custom_data          JSONB NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(custom_data) = 'object'),
    formula_errors       JSONB CHECK (formula_errors IS NULL OR jsonb_typeof(formula_errors) = 'object'),
    -- 共通欄位（§8.1）
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by           UUID REFERENCES users(id),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by           UUID REFERENCES users(id),
    deleted_at           TIMESTAMPTZ,
    version              INTEGER NOT NULL DEFAULT 1
);

COMMENT ON TABLE shipment_lines IS '出貨明細（§8.9，明細子表 shipment_line）';
COMMENT ON COLUMN shipment_lines.shipment_id IS '所屬單據（不是欄位）';
COMMENT ON COLUMN shipment_lines.sort_order IS '明細順序，從 0 開始（不是欄位）';
COMMENT ON COLUMN shipment_lines.sales_order_line_id IS '來源訂單明細（relation → sales_order_line，預設隱藏）';
COMMENT ON COLUMN shipment_lines.lot_no IS '手動指定批號：留空 = 先進先出，有值 = 只從這個批號出貨；service 不可填，由 hammer_core 檢查';
COMMENT ON COLUMN shipment_lines.cost_amount_twd IS 'round(Σ 批號扣除的 qty × unit_cost_twd, 2)，過帳時寫入；service 與草稿為 NULL';
COMMENT ON COLUMN shipment_lines.lot_id IS '手動指定的批號（不是欄位），過帳時寫入；先進先出與 service 為 NULL，以 lot_allocations 為準';

CREATE INDEX idx_shipment_lines_shipment_id
    ON shipment_lines (shipment_id, sort_order, id) WHERE deleted_at IS NULL;
CREATE INDEX idx_shipment_lines_sales_order_line_id ON shipment_lines (sales_order_line_id);
CREATE INDEX idx_shipment_lines_product_id ON shipment_lines (product_id);
CREATE INDEX idx_shipment_lines_lot_id ON shipment_lines (lot_id);
CREATE INDEX idx_shipment_lines_custom_data ON shipment_lines USING GIN (custom_data);

CREATE TRIGGER trg_shipment_lines_set_updated_at
    BEFORE UPDATE ON shipment_lines
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- sales_returns（銷貨退回，系統單據 sales_return）
-- ============================================================================

CREATE TABLE sales_returns (
    id                 UUID PRIMARY KEY,
    record_no          TEXT NOT NULL,
    shipment_id        UUID NOT NULL REFERENCES shipments(id),
    customer_id        UUID NOT NULL REFERENCES customers(id),
    return_date        DATE NOT NULL,
    tax_type           TEXT NOT NULL REFERENCES tax_rates(code),
    tax_included       BOOLEAN NOT NULL,
    status             TEXT NOT NULL DEFAULT 'draft'
                       CHECK (status IN ('draft', 'posted', 'void')),
    subtotal           NUMERIC(18,4) NOT NULL DEFAULT 0,
    tax_amount         NUMERIC(18,4) NOT NULL DEFAULT 0,
    total              NUMERIC(18,4) NOT NULL DEFAULT 0,
    cost_total_twd     NUMERIC(18,2),
    reason             TEXT,
    notes              TEXT,
    posted_at          TIMESTAMPTZ,
    posted_by          UUID REFERENCES users(id),
    voided_at          TIMESTAMPTZ,
    voided_by          UUID REFERENCES users(id),
    custom_data        JSONB NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(custom_data) = 'object'),
    formula_errors     JSONB CHECK (formula_errors IS NULL OR jsonb_typeof(formula_errors) = 'object'),
    -- 共通欄位（§8.1）
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by         UUID REFERENCES users(id),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by         UUID REFERENCES users(id),
    deleted_at         TIMESTAMPTZ,
    version            INTEGER NOT NULL DEFAULT 1,

    CONSTRAINT sales_returns_posted_status_check
        CHECK ((status = 'draft') = (posted_at IS NULL)),
    CONSTRAINT sales_returns_voided_status_check
        CHECK ((status = 'void') = (voided_at IS NOT NULL)),
    CONSTRAINT sales_returns_posted_by_check
        CHECK ((posted_at IS NULL) = (posted_by IS NULL)),
    CONSTRAINT sales_returns_voided_by_check
        CHECK ((voided_at IS NULL) = (voided_by IS NULL))
);

COMMENT ON TABLE sales_returns IS '銷貨退回（§8.9，系統單據 sales_return）';
COMMENT ON COLUMN sales_returns.record_no IS '單號（不是欄位），依 numbering_pattern 取號；唯一索引含已刪除的資料';
COMMENT ON COLUMN sales_returns.shipment_id IS '退回的出貨單（必須已過帳）；建立後不可更換';
COMMENT ON COLUMN sales_returns.customer_id IS '= 出貨單的客戶，系統維護';
COMMENT ON COLUMN sales_returns.return_date IS '退回日期：異動的 txn_date';
COMMENT ON COLUMN sales_returns.tax_included IS '= 出貨單的值，系統維護';
COMMENT ON COLUMN sales_returns.status IS 'draft / posted / void，系統維護（§7.1、§8.2）';
COMMENT ON COLUMN sales_returns.cost_total_twd IS '退回成本：明細 cost_amount_twd 的合計，過帳時寫入；草稿為 NULL';
COMMENT ON COLUMN sales_returns.posted_at IS '過帳時間（不是欄位）';
COMMENT ON COLUMN sales_returns.voided_at IS '作廢時間（不是欄位）';

CREATE UNIQUE INDEX uq_sales_returns_record_no ON sales_returns (record_no);
CREATE INDEX idx_sales_returns_return_date ON sales_returns (return_date, id);
CREATE INDEX idx_sales_returns_status ON sales_returns (status);
CREATE INDEX idx_sales_returns_customer_id ON sales_returns (customer_id);
CREATE INDEX idx_sales_returns_shipment_id ON sales_returns (shipment_id);
CREATE INDEX idx_sales_returns_custom_data ON sales_returns USING GIN (custom_data);

CREATE TRIGGER trg_sales_returns_set_updated_at
    BEFORE UPDATE ON sales_returns
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- sales_return_lines（明細子表 sales_return_line）
-- ============================================================================

CREATE TABLE sales_return_lines (
    id                 UUID PRIMARY KEY,
    sales_return_id    UUID NOT NULL REFERENCES sales_returns(id),
    sort_order         INTEGER NOT NULL DEFAULT 0,
    shipment_line_id   UUID NOT NULL REFERENCES shipment_lines(id),
    product_id         UUID NOT NULL REFERENCES products(id),
    qty                NUMERIC(18,4) NOT NULL CHECK (qty > 0),
    unit_price         NUMERIC(18,4) NOT NULL CHECK (unit_price >= 0),
    discount_rate      NUMERIC(6,4) NOT NULL DEFAULT 0 CHECK (discount_rate >= 0 AND discount_rate <= 1),
    amount             NUMERIC(18,4) NOT NULL DEFAULT 0,
    cost_amount_twd    NUMERIC(18,2),
    notes              TEXT,
    custom_data        JSONB NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(custom_data) = 'object'),
    formula_errors     JSONB CHECK (formula_errors IS NULL OR jsonb_typeof(formula_errors) = 'object'),
    -- 共通欄位（§8.1）
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by         UUID REFERENCES users(id),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by         UUID REFERENCES users(id),
    deleted_at         TIMESTAMPTZ,
    version            INTEGER NOT NULL DEFAULT 1
);

COMMENT ON TABLE sales_return_lines IS '銷貨退回明細（§8.9，明細子表 sales_return_line）';
COMMENT ON COLUMN sales_return_lines.sales_return_id IS '所屬單據（不是欄位）';
COMMENT ON COLUMN sales_return_lines.sort_order IS '明細順序，從 0 開始（不是欄位）';
COMMENT ON COLUMN sales_return_lines.shipment_line_id IS '退回的出貨明細（relation → shipment_line，預設隱藏）';
COMMENT ON COLUMN sales_return_lines.product_id IS '= 出貨明細的商品，系統維護';
COMMENT ON COLUMN sales_return_lines.unit_price IS '= 出貨明細的單價，系統維護';
COMMENT ON COLUMN sales_return_lines.discount_rate IS '= 出貨明細的折扣率，系統維護';
COMMENT ON COLUMN sales_return_lines.cost_amount_twd IS 'round(Σ 加回的 qty × unit_cost_twd, 2)，過帳時寫入；service 與草稿為 NULL';

CREATE INDEX idx_sales_return_lines_sales_return_id
    ON sales_return_lines (sales_return_id, sort_order, id) WHERE deleted_at IS NULL;
CREATE INDEX idx_sales_return_lines_shipment_line_id ON sales_return_lines (shipment_line_id);
CREATE INDEX idx_sales_return_lines_product_id ON sales_return_lines (product_id);
CREATE INDEX idx_sales_return_lines_custom_data ON sales_return_lines USING GIN (custom_data);

CREATE TRIGGER trg_sales_return_lines_set_updated_at
    BEFORE UPDATE ON sales_return_lines
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- 庫存的來源類型（§8.10）
-- ============================================================================

ALTER TABLE lot_allocations DROP CONSTRAINT lot_allocations_source_type_check;
ALTER TABLE lot_allocations ADD CONSTRAINT lot_allocations_source_type_check
    CHECK (source_type IN ('stock_adjustment_line', 'purchase_return_line', 'shipment_line'));

ALTER TABLE inventory_transactions DROP CONSTRAINT inventory_transactions_ref_doc_type_check;
ALTER TABLE inventory_transactions ADD CONSTRAINT inventory_transactions_ref_doc_type_check
    CHECK (ref_doc_type IN ('stock_adjustment', 'goods_receipt', 'purchase_return', 'shipment', 'sales_return'));

-- ============================================================================
-- 系統資料類別（§8.9「資料類別」）
-- ============================================================================

INSERT INTO entity_definitions
    (id, key, label, kind, is_system, storage, table_name, parent_entity_id,
     numbering_pattern, title_template, sort_order)
VALUES
    ('01a1f9d2-7b30-7000-94a2-592b16d9d49a', 'sales_order', '訂單', 'document', true, 'table',
     'sales_orders', NULL, 'SO{yyyyMM}-{0000}', '{record_no} {customer_id.name}', 200),
    ('01a1f9d2-7b30-7001-810e-1c4e6e7ccaee', 'sales_order_line', '訂單明細', 'detail', true, 'table',
     'sales_order_lines', '01a1f9d2-7b30-7000-94a2-592b16d9d49a', NULL, '{product_id.code} {product_id.name}', 0),
    ('01a1f9d2-7b30-7002-bbf4-a37505aebf4d', 'shipment', '出貨單', 'document', true, 'table',
     'shipments', NULL, 'SH{yyyyMM}-{0000}', '{record_no} {customer_id.name}', 210),
    ('01a1f9d2-7b30-7003-a04b-d9224e8eaf33', 'shipment_line', '出貨明細', 'detail', true, 'table',
     'shipment_lines', '01a1f9d2-7b30-7002-bbf4-a37505aebf4d', NULL, '{product_id.code} {product_id.name} {lot_no}', 0),
    ('01a1f9d2-7b30-7004-a2fd-f5fe1974d3b8', 'sales_return', '銷貨退回', 'document', true, 'table',
     'sales_returns', NULL, 'SR{yyyyMM}-{0000}', '{record_no} {customer_id.name}', 220),
    ('01a1f9d2-7b30-7005-96eb-7d816fcd7344', 'sales_return_line', '銷貨退回明細', 'detail', true, 'table',
     'sales_return_lines', '01a1f9d2-7b30-7004-a2fd-f5fe1974d3b8', NULL, '{product_id.code} {product_id.name}', 0);

-- ============================================================================
-- 系統欄位（sort_order 依表中順序 10、20、30…；唯讀的為系統維護的欄位，§7.1）
-- ============================================================================

-- sales_order
INSERT INTO field_definitions
    (id, entity_id, field_key, label, data_type, is_system, storage, column_name,
     is_required, is_visible, is_readonly, default_value, validation, options,
     decimal_places, relation_entity_id, ui_widget, sort_order)
VALUES
    ('01a1f9d2-7b30-7006-8110-fe0452675a11', '01a1f9d2-7b30-7000-94a2-592b16d9d49a', 'customer_id', '客戶', 'relation', true, 'column', 'customer_id',
     true, true, false, NULL, NULL, NULL,
     NULL, '01a1e516-3404-7153-bc28-b536c04db4db', NULL, 10),
    ('01a1f9d2-7b30-7007-9a04-1036fd7ffe13', '01a1f9d2-7b30-7000-94a2-592b16d9d49a', 'order_date', '訂單日期', 'date', true, 'column', 'order_date',
     true, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 20),
    ('01a1f9d2-7b30-7008-a4f5-dfd7c4fff7eb', '01a1f9d2-7b30-7000-94a2-592b16d9d49a', 'delivery_date', '預計交貨日', 'date', true, 'column', 'delivery_date',
     false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 30),
    ('01a1f9d2-7b30-7009-9b49-0111c94d17ce', '01a1f9d2-7b30-7000-94a2-592b16d9d49a', 'tax_type', '稅別', 'select', true, 'column', 'tax_type',
     true, true, false, NULL, NULL, '[{"value":"taxable","label":"應稅"},{"value":"zero_rated","label":"零稅率"},{"value":"exempt","label":"免稅"}]'::jsonb,
     NULL, NULL, NULL, 40),
    ('01a1f9d2-7b30-700a-9166-039996013338', '01a1f9d2-7b30-7000-94a2-592b16d9d49a', 'tax_included', '單價含稅', 'boolean', true, 'column', 'tax_included',
     true, true, false, 'false'::jsonb, NULL, NULL,
     NULL, NULL, NULL, 50),
    ('01a1f9d2-7b30-700b-bd8c-fc045e455cc2', '01a1f9d2-7b30-7000-94a2-592b16d9d49a', 'status', '狀態', 'select', true, 'column', 'status',
     false, true, true, NULL, NULL, '[{"value":"draft","label":"草稿"},{"value":"confirmed","label":"已確認"},{"value":"partially_shipped","label":"部分出貨"},{"value":"closed","label":"已結案"},{"value":"void","label":"已作廢"}]'::jsonb,
     NULL, NULL, NULL, 60),
    ('01a1f9d2-7b30-700c-aaa0-961df68a40d6', '01a1f9d2-7b30-7000-94a2-592b16d9d49a', 'subtotal', '未稅金額', 'money', true, 'column', 'subtotal',
     false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 70),
    ('01a1f9d2-7b30-700d-b2ce-bbe12bb3a50d', '01a1f9d2-7b30-7000-94a2-592b16d9d49a', 'tax_amount', '稅額', 'money', true, 'column', 'tax_amount',
     false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 80),
    ('01a1f9d2-7b30-700e-b8bf-922ba064fa41', '01a1f9d2-7b30-7000-94a2-592b16d9d49a', 'total', '總計', 'money', true, 'column', 'total',
     false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 90),
    ('01a1f9d2-7b30-700f-8f0a-d7331e06cfa7', '01a1f9d2-7b30-7000-94a2-592b16d9d49a', 'notes', '備註', 'long_text', true, 'column', 'notes',
     false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 100);

-- sales_order_line
INSERT INTO field_definitions
    (id, entity_id, field_key, label, data_type, is_system, storage, column_name,
     is_required, is_visible, is_readonly, default_value, validation, options,
     decimal_places, relation_entity_id, ui_widget, sort_order)
VALUES
    ('01a1f9d2-7b30-7010-9fb7-edb7e8962971', '01a1f9d2-7b30-7001-810e-1c4e6e7ccaee', 'product_id', '商品', 'relation', true, 'column', 'product_id',
     true, true, false, NULL, NULL, NULL,
     NULL, '01a1e516-3401-7432-acb3-9cc3ac6a7db9', NULL, 10),
    ('01a1f9d2-7b30-7011-bb95-0b5f91039176', '01a1f9d2-7b30-7001-810e-1c4e6e7ccaee', 'qty', '數量', 'decimal', true, 'column', 'qty',
     true, true, false, NULL, '{"min":"0"}'::jsonb, NULL,
     4, NULL, NULL, 20),
    ('01a1f9d2-7b30-7012-8b1c-058c6ba3870c', '01a1f9d2-7b30-7001-810e-1c4e6e7ccaee', 'unit_price', '單價', 'money', true, 'column', 'unit_price',
     true, true, false, NULL, '{"min":"0"}'::jsonb, NULL,
     4, NULL, NULL, 30),
    ('01a1f9d2-7b30-7013-b1a2-d24b51cc1086', '01a1f9d2-7b30-7001-810e-1c4e6e7ccaee', 'discount_rate', '折扣率', 'decimal', true, 'column', 'discount_rate',
     true, true, false, '"0"'::jsonb, '{"min":"0","max":"1"}'::jsonb, NULL,
     4, NULL, NULL, 40),
    ('01a1f9d2-7b30-7014-87d3-7e457630e107', '01a1f9d2-7b30-7001-810e-1c4e6e7ccaee', 'amount', '金額', 'money', true, 'column', 'amount',
     false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 50),
    ('01a1f9d2-7b30-7015-916c-1e50990a73be', '01a1f9d2-7b30-7001-810e-1c4e6e7ccaee', 'shipped_qty', '已出貨數量', 'decimal', true, 'column', 'shipped_qty',
     false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 60),
    ('01a1f9d2-7b30-7016-96cd-d2db22c5973c', '01a1f9d2-7b30-7001-810e-1c4e6e7ccaee', 'notes', '備註', 'long_text', true, 'column', 'notes',
     false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 70);

-- shipment
INSERT INTO field_definitions
    (id, entity_id, field_key, label, data_type, is_system, storage, column_name,
     is_required, is_visible, is_readonly, default_value, validation, options,
     decimal_places, relation_entity_id, ui_widget, sort_order)
VALUES
    ('01a1f9d2-7b30-7017-85a0-46de7c669d9b', '01a1f9d2-7b30-7002-bbf4-a37505aebf4d', 'customer_id', '客戶', 'relation', true, 'column', 'customer_id',
     true, true, false, NULL, NULL, NULL,
     NULL, '01a1e516-3404-7153-bc28-b536c04db4db', NULL, 10),
    ('01a1f9d2-7b30-7018-bbed-eccb5ba7a462', '01a1f9d2-7b30-7002-bbf4-a37505aebf4d', 'sales_order_id', '訂單', 'relation', true, 'column', 'sales_order_id',
     false, true, false, NULL, NULL, NULL,
     NULL, '01a1f9d2-7b30-7000-94a2-592b16d9d49a', NULL, 20),
    ('01a1f9d2-7b30-7019-887a-88cfaf103375', '01a1f9d2-7b30-7002-bbf4-a37505aebf4d', 'ship_date', '出貨日期', 'date', true, 'column', 'ship_date',
     true, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 30),
    ('01a1f9d2-7b30-701a-9c8f-2e9a7eb7a5ae', '01a1f9d2-7b30-7002-bbf4-a37505aebf4d', 'tax_type', '稅別', 'select', true, 'column', 'tax_type',
     true, true, false, NULL, NULL, '[{"value":"taxable","label":"應稅"},{"value":"zero_rated","label":"零稅率"},{"value":"exempt","label":"免稅"}]'::jsonb,
     NULL, NULL, NULL, 40),
    ('01a1f9d2-7b30-701b-b39e-0939f280d0d7', '01a1f9d2-7b30-7002-bbf4-a37505aebf4d', 'tax_included', '單價含稅', 'boolean', true, 'column', 'tax_included',
     true, true, false, 'false'::jsonb, NULL, NULL,
     NULL, NULL, NULL, 50),
    ('01a1f9d2-7b30-701c-9f72-a1c9047deeb0', '01a1f9d2-7b30-7002-bbf4-a37505aebf4d', 'status', '狀態', 'select', true, 'column', 'status',
     false, true, true, NULL, NULL, '[{"value":"draft","label":"草稿"},{"value":"posted","label":"已過帳"},{"value":"void","label":"已作廢"}]'::jsonb,
     NULL, NULL, NULL, 60),
    ('01a1f9d2-7b30-701d-8397-a2a0acb31ae1', '01a1f9d2-7b30-7002-bbf4-a37505aebf4d', 'subtotal', '未稅金額', 'money', true, 'column', 'subtotal',
     false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 70),
    ('01a1f9d2-7b30-701e-b801-43463dc97379', '01a1f9d2-7b30-7002-bbf4-a37505aebf4d', 'tax_amount', '稅額', 'money', true, 'column', 'tax_amount',
     false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 80),
    ('01a1f9d2-7b30-701f-97e3-36dbdf3d458d', '01a1f9d2-7b30-7002-bbf4-a37505aebf4d', 'total', '總計', 'money', true, 'column', 'total',
     false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 90),
    ('01a1f9d2-7b30-7020-ac64-aa5b52bba152', '01a1f9d2-7b30-7002-bbf4-a37505aebf4d', 'cost_total_twd', '銷貨成本', 'money', true, 'column', 'cost_total_twd',
     false, true, true, NULL, NULL, NULL,
     2, NULL, NULL, 100),
    ('01a1f9d2-7b30-7021-981c-f2946339fca5', '01a1f9d2-7b30-7002-bbf4-a37505aebf4d', 'ship_address', '送貨地址', 'text', true, 'column', 'ship_address',
     false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 110),
    ('01a1f9d2-7b30-7022-932d-993b985a9c5a', '01a1f9d2-7b30-7002-bbf4-a37505aebf4d', 'notes', '備註', 'long_text', true, 'column', 'notes',
     false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 120);

-- shipment_line
INSERT INTO field_definitions
    (id, entity_id, field_key, label, data_type, is_system, storage, column_name,
     is_required, is_visible, is_readonly, default_value, validation, options,
     decimal_places, relation_entity_id, ui_widget, sort_order)
VALUES
    ('01a1f9d2-7b30-7023-bc2e-186a951be88a', '01a1f9d2-7b30-7003-a04b-d9224e8eaf33', 'sales_order_line_id', '訂單明細', 'relation', true, 'column', 'sales_order_line_id',
     false, false, false, NULL, NULL, NULL,
     NULL, '01a1f9d2-7b30-7001-810e-1c4e6e7ccaee', NULL, 10),
    ('01a1f9d2-7b30-7024-9732-0b958c22c883', '01a1f9d2-7b30-7003-a04b-d9224e8eaf33', 'product_id', '商品', 'relation', true, 'column', 'product_id',
     true, true, false, NULL, NULL, NULL,
     NULL, '01a1e516-3401-7432-acb3-9cc3ac6a7db9', NULL, 20),
    ('01a1f9d2-7b30-7025-b20d-cde7cc5a8499', '01a1f9d2-7b30-7003-a04b-d9224e8eaf33', 'qty', '數量', 'decimal', true, 'column', 'qty',
     true, true, false, NULL, '{"min":"0"}'::jsonb, NULL,
     4, NULL, NULL, 30),
    ('01a1f9d2-7b30-7026-a15d-de74aabb03fc', '01a1f9d2-7b30-7003-a04b-d9224e8eaf33', 'unit_price', '單價', 'money', true, 'column', 'unit_price',
     true, true, false, NULL, '{"min":"0"}'::jsonb, NULL,
     4, NULL, NULL, 40),
    ('01a1f9d2-7b30-7027-bb45-05fd11133d70', '01a1f9d2-7b30-7003-a04b-d9224e8eaf33', 'discount_rate', '折扣率', 'decimal', true, 'column', 'discount_rate',
     true, true, false, '"0"'::jsonb, '{"min":"0","max":"1"}'::jsonb, NULL,
     4, NULL, NULL, 50),
    ('01a1f9d2-7b30-7028-a163-e9951dd68b03', '01a1f9d2-7b30-7003-a04b-d9224e8eaf33', 'amount', '金額', 'money', true, 'column', 'amount',
     false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 60),
    ('01a1f9d2-7b30-7029-96c1-d2647b3e6e82', '01a1f9d2-7b30-7003-a04b-d9224e8eaf33', 'lot_no', '批號', 'text', true, 'column', 'lot_no',
     false, true, false, NULL, '{"max_length":50}'::jsonb, NULL,
     NULL, NULL, NULL, 70),
    ('01a1f9d2-7b30-702a-8a77-9c2279d3a9e7', '01a1f9d2-7b30-7003-a04b-d9224e8eaf33', 'cost_amount_twd', '成本', 'money', true, 'column', 'cost_amount_twd',
     false, true, true, NULL, NULL, NULL,
     2, NULL, NULL, 80),
    ('01a1f9d2-7b30-702b-bbb7-903a50b4bfe9', '01a1f9d2-7b30-7003-a04b-d9224e8eaf33', 'notes', '備註', 'long_text', true, 'column', 'notes',
     false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 90);

-- sales_return
INSERT INTO field_definitions
    (id, entity_id, field_key, label, data_type, is_system, storage, column_name,
     is_required, is_visible, is_readonly, default_value, validation, options,
     decimal_places, relation_entity_id, ui_widget, sort_order)
VALUES
    ('01a1f9d2-7b30-702c-92dc-8834708f7299', '01a1f9d2-7b30-7004-a2fd-f5fe1974d3b8', 'shipment_id', '出貨單', 'relation', true, 'column', 'shipment_id',
     true, true, false, NULL, NULL, NULL,
     NULL, '01a1f9d2-7b30-7002-bbf4-a37505aebf4d', NULL, 10),
    ('01a1f9d2-7b30-702d-98a5-eaf50b70259c', '01a1f9d2-7b30-7004-a2fd-f5fe1974d3b8', 'customer_id', '客戶', 'relation', true, 'column', 'customer_id',
     false, true, true, NULL, NULL, NULL,
     NULL, '01a1e516-3404-7153-bc28-b536c04db4db', NULL, 20),
    ('01a1f9d2-7b30-702e-96ca-8e42ac9287e5', '01a1f9d2-7b30-7004-a2fd-f5fe1974d3b8', 'return_date', '退回日期', 'date', true, 'column', 'return_date',
     true, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 30),
    ('01a1f9d2-7b30-702f-8585-8a314ce9e425', '01a1f9d2-7b30-7004-a2fd-f5fe1974d3b8', 'tax_type', '稅別', 'select', true, 'column', 'tax_type',
     false, true, true, NULL, NULL, '[{"value":"taxable","label":"應稅"},{"value":"zero_rated","label":"零稅率"},{"value":"exempt","label":"免稅"}]'::jsonb,
     NULL, NULL, NULL, 40),
    ('01a1f9d2-7b30-7030-a1db-c33c91243b3c', '01a1f9d2-7b30-7004-a2fd-f5fe1974d3b8', 'tax_included', '單價含稅', 'boolean', true, 'column', 'tax_included',
     false, true, true, NULL, NULL, NULL,
     NULL, NULL, NULL, 50),
    ('01a1f9d2-7b30-7031-8698-3495da4bf6a3', '01a1f9d2-7b30-7004-a2fd-f5fe1974d3b8', 'status', '狀態', 'select', true, 'column', 'status',
     false, true, true, NULL, NULL, '[{"value":"draft","label":"草稿"},{"value":"posted","label":"已過帳"},{"value":"void","label":"已作廢"}]'::jsonb,
     NULL, NULL, NULL, 60),
    ('01a1f9d2-7b30-7032-8530-ac8830862e44', '01a1f9d2-7b30-7004-a2fd-f5fe1974d3b8', 'subtotal', '未稅金額', 'money', true, 'column', 'subtotal',
     false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 70),
    ('01a1f9d2-7b30-7033-a794-ae6eab4bc532', '01a1f9d2-7b30-7004-a2fd-f5fe1974d3b8', 'tax_amount', '稅額', 'money', true, 'column', 'tax_amount',
     false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 80),
    ('01a1f9d2-7b30-7034-90bc-7381969cb743', '01a1f9d2-7b30-7004-a2fd-f5fe1974d3b8', 'total', '總計', 'money', true, 'column', 'total',
     false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 90),
    ('01a1f9d2-7b30-7035-85ca-50c3ce81defc', '01a1f9d2-7b30-7004-a2fd-f5fe1974d3b8', 'cost_total_twd', '退回成本', 'money', true, 'column', 'cost_total_twd',
     false, true, true, NULL, NULL, NULL,
     2, NULL, NULL, 100),
    ('01a1f9d2-7b30-7036-8283-a2f29da5ca5f', '01a1f9d2-7b30-7004-a2fd-f5fe1974d3b8', 'reason', '退回原因', 'text', true, 'column', 'reason',
     false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 110),
    ('01a1f9d2-7b30-7037-9286-b036c3f368a8', '01a1f9d2-7b30-7004-a2fd-f5fe1974d3b8', 'notes', '備註', 'long_text', true, 'column', 'notes',
     false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 120);

-- sales_return_line
INSERT INTO field_definitions
    (id, entity_id, field_key, label, data_type, is_system, storage, column_name,
     is_required, is_visible, is_readonly, default_value, validation, options,
     decimal_places, relation_entity_id, ui_widget, sort_order)
VALUES
    ('01a1f9d2-7b30-7038-9f37-46a1fd8a3649', '01a1f9d2-7b30-7005-96eb-7d816fcd7344', 'shipment_line_id', '出貨明細', 'relation', true, 'column', 'shipment_line_id',
     true, false, false, NULL, NULL, NULL,
     NULL, '01a1f9d2-7b30-7003-a04b-d9224e8eaf33', NULL, 10),
    ('01a1f9d2-7b30-7039-8b50-a82d0f1e9465', '01a1f9d2-7b30-7005-96eb-7d816fcd7344', 'product_id', '商品', 'relation', true, 'column', 'product_id',
     false, true, true, NULL, NULL, NULL,
     NULL, '01a1e516-3401-7432-acb3-9cc3ac6a7db9', NULL, 20),
    ('01a1f9d2-7b30-703a-83d1-beceb99f471e', '01a1f9d2-7b30-7005-96eb-7d816fcd7344', 'qty', '數量', 'decimal', true, 'column', 'qty',
     true, true, false, NULL, '{"min":"0"}'::jsonb, NULL,
     4, NULL, NULL, 30),
    ('01a1f9d2-7b30-703b-9cca-c4ed42165226', '01a1f9d2-7b30-7005-96eb-7d816fcd7344', 'unit_price', '單價', 'money', true, 'column', 'unit_price',
     false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 40),
    ('01a1f9d2-7b30-703c-a14a-a8746e4b3646', '01a1f9d2-7b30-7005-96eb-7d816fcd7344', 'discount_rate', '折扣率', 'decimal', true, 'column', 'discount_rate',
     false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 50),
    ('01a1f9d2-7b30-703d-af87-514439871884', '01a1f9d2-7b30-7005-96eb-7d816fcd7344', 'amount', '金額', 'money', true, 'column', 'amount',
     false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 60),
    ('01a1f9d2-7b30-703e-80d7-f5c0f2739e62', '01a1f9d2-7b30-7005-96eb-7d816fcd7344', 'cost_amount_twd', '成本', 'money', true, 'column', 'cost_amount_twd',
     false, true, true, NULL, NULL, NULL,
     2, NULL, NULL, 70),
    ('01a1f9d2-7b30-703f-82e1-4590b67ebe5a', '01a1f9d2-7b30-7005-96eb-7d816fcd7344', 'notes', '備註', 'long_text', true, 'column', 'notes',
     false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 80);


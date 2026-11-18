-- 0012_purchasing：M4b 採購單、進貨單、進貨退回的資料表與資料類別（#138，docs/architecture.md §8.8）。
--
-- 建立三張系統單據與其明細子表，依 §8.6「系統資料表的共同規則」與 §7.1「系統單據」：
-- 系統欄位 → custom_data → formula_errors → 共通欄位；單號 record_no 與確認（過帳）、作廢、結案的時間與人員、
-- 明細的 lot_id 不是欄位。
--
-- 系統維護的欄位（status、金額合計、amount、received_qty、unit_cost_twd 等）是唯讀的系統欄位；
-- 金額欄位以 DEFAULT 0 建立，由 hammer_core 在存檔時計算（#139）。
--
-- 庫存三張表的 source_type、ref_doc_type 加上進貨單與進貨退回的代碼（§8.10）。
--
-- 資料類別、欄位定義的 id 為固定的 uuid v7，方便跨環境比對。
-- 欄位定義必須與 architecture.md §8.8 一致，由 crates/db/tests/purchasing.rs 檢查。

-- ============================================================================
-- purchase_orders（採購單，系統單據 purchase_order）
-- ============================================================================

CREATE TABLE purchase_orders (
    id               UUID PRIMARY KEY,
    record_no        TEXT NOT NULL,
    supplier_id      UUID NOT NULL REFERENCES suppliers(id),
    order_date       DATE NOT NULL,
    expected_date    DATE,
    currency_code    TEXT NOT NULL REFERENCES currencies(code),
    exchange_rate    NUMERIC(18,6) CHECK (exchange_rate > 0),
    tax_type         TEXT NOT NULL REFERENCES tax_rates(code),
    status           TEXT NOT NULL DEFAULT 'draft'
                     CHECK (status IN ('draft', 'confirmed', 'partially_received', 'closed', 'void')),
    subtotal         NUMERIC(18,4) NOT NULL DEFAULT 0,
    tax_amount       NUMERIC(18,4) NOT NULL DEFAULT 0,
    total            NUMERIC(18,4) NOT NULL DEFAULT 0,
    total_twd        NUMERIC(18,2),
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

    CONSTRAINT purchase_orders_posted_status_check
        CHECK ((status = 'draft') = (posted_at IS NULL)),
    CONSTRAINT purchase_orders_voided_status_check
        CHECK ((status = 'void') = (voided_at IS NOT NULL)),
    CONSTRAINT purchase_orders_posted_by_check
        CHECK ((posted_at IS NULL) = (posted_by IS NULL)),
    CONSTRAINT purchase_orders_voided_by_check
        CHECK ((voided_at IS NULL) = (voided_by IS NULL)),
    CONSTRAINT purchase_orders_closed_by_check
        CHECK ((closed_at IS NULL) = (closed_by IS NULL)),
    CONSTRAINT purchase_orders_closed_status_check
        CHECK (closed_at IS NULL OR status = 'closed'),
    -- 確認後必須有匯率（§8.8「幣別與匯率」）
    CONSTRAINT purchase_orders_confirmed_rate_check
        CHECK (status = 'draft' OR exchange_rate IS NOT NULL)
);

COMMENT ON TABLE purchase_orders IS '採購單（§8.8，系統單據 purchase_order）';
COMMENT ON COLUMN purchase_orders.record_no IS '單號（不是欄位），依 numbering_pattern 取號；唯一索引含已刪除的資料';
COMMENT ON COLUMN purchase_orders.order_date IS '採購日期：匯率的適用日期';
COMMENT ON COLUMN purchase_orders.exchange_rate IS '1 外幣 = ? 台幣；草稿可以留空，確認時必填';
COMMENT ON COLUMN purchase_orders.status IS 'draft / confirmed / partially_received / closed / void，系統維護（§8.8「採購單的狀態」）';
COMMENT ON COLUMN purchase_orders.subtotal IS '未稅金額（單據幣別），系統維護';
COMMENT ON COLUMN purchase_orders.total_twd IS '台幣總計（參考用），匯率留空時為 NULL';
COMMENT ON COLUMN purchase_orders.posted_at IS '確認時間（不是欄位）';
COMMENT ON COLUMN purchase_orders.voided_at IS '作廢時間（不是欄位）';
COMMENT ON COLUMN purchase_orders.closed_at IS '手動結案時間（不是欄位）；自動結案時為 NULL';

CREATE UNIQUE INDEX uq_purchase_orders_record_no ON purchase_orders (record_no);
CREATE INDEX idx_purchase_orders_order_date ON purchase_orders (order_date, id);
CREATE INDEX idx_purchase_orders_status ON purchase_orders (status);
CREATE INDEX idx_purchase_orders_supplier_id ON purchase_orders (supplier_id);
CREATE INDEX idx_purchase_orders_currency_code ON purchase_orders (currency_code);
CREATE INDEX idx_purchase_orders_custom_data ON purchase_orders USING GIN (custom_data);

CREATE TRIGGER trg_purchase_orders_set_updated_at
    BEFORE UPDATE ON purchase_orders
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- purchase_order_lines（明細子表 purchase_order_line）
-- ============================================================================

CREATE TABLE purchase_order_lines (
    id                 UUID PRIMARY KEY,
    purchase_order_id  UUID NOT NULL REFERENCES purchase_orders(id),
    sort_order         INTEGER NOT NULL DEFAULT 0,
    product_id         UUID NOT NULL REFERENCES products(id),
    qty                NUMERIC(18,4) NOT NULL CHECK (qty > 0),
    unit_price         NUMERIC(18,4) NOT NULL CHECK (unit_price >= 0),
    amount             NUMERIC(18,4) NOT NULL DEFAULT 0,
    received_qty       NUMERIC(18,4) NOT NULL DEFAULT 0 CHECK (received_qty >= 0),
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

COMMENT ON TABLE purchase_order_lines IS '採購明細（§8.8，明細子表 purchase_order_line）';
COMMENT ON COLUMN purchase_order_lines.purchase_order_id IS '所屬單據（不是欄位）';
COMMENT ON COLUMN purchase_order_lines.sort_order IS '明細順序，從 0 開始（不是欄位）';
COMMENT ON COLUMN purchase_order_lines.unit_price IS '單價（單據幣別、未稅）';
COMMENT ON COLUMN purchase_order_lines.amount IS 'round(qty × unit_price, 幣別小數位數)，系統維護';
COMMENT ON COLUMN purchase_order_lines.received_qty IS '已進貨數量：進貨單過帳時增加、作廢時減少，系統維護';

CREATE INDEX idx_purchase_order_lines_purchase_order_id
    ON purchase_order_lines (purchase_order_id, sort_order, id) WHERE deleted_at IS NULL;
CREATE INDEX idx_purchase_order_lines_product_id ON purchase_order_lines (product_id);
CREATE INDEX idx_purchase_order_lines_custom_data ON purchase_order_lines USING GIN (custom_data);

CREATE TRIGGER trg_purchase_order_lines_set_updated_at
    BEFORE UPDATE ON purchase_order_lines
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- goods_receipts（進貨單，系統單據 goods_receipt）
-- ============================================================================

CREATE TABLE goods_receipts (
    id                 UUID PRIMARY KEY,
    record_no          TEXT NOT NULL,
    supplier_id        UUID NOT NULL REFERENCES suppliers(id),
    purchase_order_id  UUID REFERENCES purchase_orders(id),
    receipt_date       DATE NOT NULL,
    currency_code      TEXT NOT NULL REFERENCES currencies(code),
    exchange_rate      NUMERIC(18,6) CHECK (exchange_rate > 0),
    tax_type           TEXT NOT NULL REFERENCES tax_rates(code),
    status             TEXT NOT NULL DEFAULT 'draft'
                       CHECK (status IN ('draft', 'posted', 'void')),
    subtotal           NUMERIC(18,4) NOT NULL DEFAULT 0,
    tax_amount         NUMERIC(18,4) NOT NULL DEFAULT 0,
    total              NUMERIC(18,4) NOT NULL DEFAULT 0,
    total_twd          NUMERIC(18,2),
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

    CONSTRAINT goods_receipts_posted_status_check
        CHECK ((status = 'draft') = (posted_at IS NULL)),
    CONSTRAINT goods_receipts_voided_status_check
        CHECK ((status = 'void') = (voided_at IS NOT NULL)),
    CONSTRAINT goods_receipts_posted_by_check
        CHECK ((posted_at IS NULL) = (posted_by IS NULL)),
    CONSTRAINT goods_receipts_voided_by_check
        CHECK ((voided_at IS NULL) = (voided_by IS NULL)),
    -- 過帳後必須有匯率（§8.8「幣別與匯率」）
    CONSTRAINT goods_receipts_posted_rate_check
        CHECK (status = 'draft' OR exchange_rate IS NOT NULL)
);

COMMENT ON TABLE goods_receipts IS '進貨單（§8.8，系統單據 goods_receipt）';
COMMENT ON COLUMN goods_receipts.record_no IS '單號（不是欄位），依 numbering_pattern 取號；唯一索引含已刪除的資料';
COMMENT ON COLUMN goods_receipts.purchase_order_id IS '來源採購單；可以不經採購單直接進貨';
COMMENT ON COLUMN goods_receipts.receipt_date IS '進貨日期：匯率的適用日期、批號的 received_date、異動的 txn_date';
COMMENT ON COLUMN goods_receipts.exchange_rate IS '1 外幣 = ? 台幣；草稿可以留空，過帳時必填';
COMMENT ON COLUMN goods_receipts.status IS 'draft / posted / void，系統維護（§7.1、§8.2）';
COMMENT ON COLUMN goods_receipts.posted_at IS '過帳時間（不是欄位）';
COMMENT ON COLUMN goods_receipts.voided_at IS '作廢時間（不是欄位）';

CREATE UNIQUE INDEX uq_goods_receipts_record_no ON goods_receipts (record_no);
CREATE INDEX idx_goods_receipts_receipt_date ON goods_receipts (receipt_date, id);
CREATE INDEX idx_goods_receipts_status ON goods_receipts (status);
CREATE INDEX idx_goods_receipts_supplier_id ON goods_receipts (supplier_id);
CREATE INDEX idx_goods_receipts_purchase_order_id ON goods_receipts (purchase_order_id);
CREATE INDEX idx_goods_receipts_currency_code ON goods_receipts (currency_code);
CREATE INDEX idx_goods_receipts_custom_data ON goods_receipts USING GIN (custom_data);

CREATE TRIGGER trg_goods_receipts_set_updated_at
    BEFORE UPDATE ON goods_receipts
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- goods_receipt_lines（明細子表 goods_receipt_line）
-- ============================================================================

CREATE TABLE goods_receipt_lines (
    id                      UUID PRIMARY KEY,
    goods_receipt_id        UUID NOT NULL REFERENCES goods_receipts(id),
    sort_order              INTEGER NOT NULL DEFAULT 0,
    purchase_order_line_id  UUID REFERENCES purchase_order_lines(id),
    product_id              UUID NOT NULL REFERENCES products(id),
    qty                     NUMERIC(18,4) NOT NULL CHECK (qty > 0),
    unit_price              NUMERIC(18,4) NOT NULL CHECK (unit_price >= 0),
    amount                  NUMERIC(18,4) NOT NULL DEFAULT 0,
    lot_no                  TEXT,
    unit_cost_twd           NUMERIC(18,4) CHECK (unit_cost_twd >= 0),
    notes                   TEXT,
    lot_id                  UUID REFERENCES inventory_lots(id),
    custom_data             JSONB NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(custom_data) = 'object'),
    formula_errors          JSONB CHECK (formula_errors IS NULL OR jsonb_typeof(formula_errors) = 'object'),
    -- 共通欄位（§8.1）
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by              UUID REFERENCES users(id),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by              UUID REFERENCES users(id),
    deleted_at              TIMESTAMPTZ,
    version                 INTEGER NOT NULL DEFAULT 1
);

COMMENT ON TABLE goods_receipt_lines IS '進貨明細（§8.8，明細子表 goods_receipt_line）';
COMMENT ON COLUMN goods_receipt_lines.goods_receipt_id IS '所屬單據（不是欄位）';
COMMENT ON COLUMN goods_receipt_lines.sort_order IS '明細順序，從 0 開始（不是欄位）';
COMMENT ON COLUMN goods_receipt_lines.purchase_order_line_id IS '來源採購明細（relation → purchase_order_line，預設隱藏）';
COMMENT ON COLUMN goods_receipt_lines.unit_price IS '單價（單據幣別、未稅）';
COMMENT ON COLUMN goods_receipt_lines.lot_no IS 'goods 可留空（過帳時自動產生）；service 不可填，由 hammer_core 檢查';
COMMENT ON COLUMN goods_receipt_lines.unit_cost_twd IS 'round(unit_price × exchange_rate, 4)，過帳時寫入';
COMMENT ON COLUMN goods_receipt_lines.lot_id IS '過帳時建立的批號（不是欄位）；service 為 NULL';

CREATE INDEX idx_goods_receipt_lines_goods_receipt_id
    ON goods_receipt_lines (goods_receipt_id, sort_order, id) WHERE deleted_at IS NULL;
CREATE INDEX idx_goods_receipt_lines_purchase_order_line_id ON goods_receipt_lines (purchase_order_line_id);
CREATE INDEX idx_goods_receipt_lines_product_id ON goods_receipt_lines (product_id);
CREATE INDEX idx_goods_receipt_lines_lot_id ON goods_receipt_lines (lot_id);
CREATE INDEX idx_goods_receipt_lines_custom_data ON goods_receipt_lines USING GIN (custom_data);

CREATE TRIGGER trg_goods_receipt_lines_set_updated_at
    BEFORE UPDATE ON goods_receipt_lines
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- purchase_returns（進貨退回，系統單據 purchase_return）
-- ============================================================================

CREATE TABLE purchase_returns (
    id                 UUID PRIMARY KEY,
    record_no          TEXT NOT NULL,
    goods_receipt_id   UUID NOT NULL REFERENCES goods_receipts(id),
    supplier_id        UUID NOT NULL REFERENCES suppliers(id),
    return_date        DATE NOT NULL,
    currency_code      TEXT NOT NULL REFERENCES currencies(code),
    exchange_rate      NUMERIC(18,6) NOT NULL CHECK (exchange_rate > 0),
    tax_type           TEXT NOT NULL REFERENCES tax_rates(code),
    status             TEXT NOT NULL DEFAULT 'draft'
                       CHECK (status IN ('draft', 'posted', 'void')),
    subtotal           NUMERIC(18,4) NOT NULL DEFAULT 0,
    tax_amount         NUMERIC(18,4) NOT NULL DEFAULT 0,
    total              NUMERIC(18,4) NOT NULL DEFAULT 0,
    total_twd          NUMERIC(18,2) NOT NULL DEFAULT 0,
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

    CONSTRAINT purchase_returns_posted_status_check
        CHECK ((status = 'draft') = (posted_at IS NULL)),
    CONSTRAINT purchase_returns_voided_status_check
        CHECK ((status = 'void') = (voided_at IS NOT NULL)),
    CONSTRAINT purchase_returns_posted_by_check
        CHECK ((posted_at IS NULL) = (posted_by IS NULL)),
    CONSTRAINT purchase_returns_voided_by_check
        CHECK ((voided_at IS NULL) = (voided_by IS NULL))
);

COMMENT ON TABLE purchase_returns IS '進貨退回（§8.8，系統單據 purchase_return）';
COMMENT ON COLUMN purchase_returns.record_no IS '單號（不是欄位），依 numbering_pattern 取號；唯一索引含已刪除的資料';
COMMENT ON COLUMN purchase_returns.goods_receipt_id IS '退回的進貨單（必須已過帳）；建立後不可更換';
COMMENT ON COLUMN purchase_returns.supplier_id IS '= 進貨單的廠商，系統維護';
COMMENT ON COLUMN purchase_returns.return_date IS '退回日期：異動的 txn_date';
COMMENT ON COLUMN purchase_returns.exchange_rate IS '= 進貨單的匯率，系統維護';
COMMENT ON COLUMN purchase_returns.status IS 'draft / posted / void，系統維護（§7.1、§8.2）';
COMMENT ON COLUMN purchase_returns.posted_at IS '過帳時間（不是欄位）';
COMMENT ON COLUMN purchase_returns.voided_at IS '作廢時間（不是欄位）';

CREATE UNIQUE INDEX uq_purchase_returns_record_no ON purchase_returns (record_no);
CREATE INDEX idx_purchase_returns_return_date ON purchase_returns (return_date, id);
CREATE INDEX idx_purchase_returns_status ON purchase_returns (status);
CREATE INDEX idx_purchase_returns_supplier_id ON purchase_returns (supplier_id);
CREATE INDEX idx_purchase_returns_goods_receipt_id ON purchase_returns (goods_receipt_id);
CREATE INDEX idx_purchase_returns_currency_code ON purchase_returns (currency_code);
CREATE INDEX idx_purchase_returns_custom_data ON purchase_returns USING GIN (custom_data);

CREATE TRIGGER trg_purchase_returns_set_updated_at
    BEFORE UPDATE ON purchase_returns
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- purchase_return_lines（明細子表 purchase_return_line）
-- ============================================================================

CREATE TABLE purchase_return_lines (
    id                     UUID PRIMARY KEY,
    purchase_return_id     UUID NOT NULL REFERENCES purchase_returns(id),
    sort_order             INTEGER NOT NULL DEFAULT 0,
    goods_receipt_line_id  UUID NOT NULL REFERENCES goods_receipt_lines(id),
    product_id             UUID NOT NULL REFERENCES products(id),
    lot_no                 TEXT,
    qty                    NUMERIC(18,4) NOT NULL CHECK (qty > 0),
    unit_price             NUMERIC(18,4) NOT NULL CHECK (unit_price >= 0),
    amount                 NUMERIC(18,4) NOT NULL DEFAULT 0,
    unit_cost_twd          NUMERIC(18,4) CHECK (unit_cost_twd >= 0),
    notes                  TEXT,
    lot_id                 UUID REFERENCES inventory_lots(id),
    custom_data            JSONB NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(custom_data) = 'object'),
    formula_errors         JSONB CHECK (formula_errors IS NULL OR jsonb_typeof(formula_errors) = 'object'),
    -- 共通欄位（§8.1）
    created_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by             UUID REFERENCES users(id),
    updated_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by             UUID REFERENCES users(id),
    deleted_at             TIMESTAMPTZ,
    version                INTEGER NOT NULL DEFAULT 1
);

COMMENT ON TABLE purchase_return_lines IS '進貨退回明細（§8.8，明細子表 purchase_return_line）';
COMMENT ON COLUMN purchase_return_lines.purchase_return_id IS '所屬單據（不是欄位）';
COMMENT ON COLUMN purchase_return_lines.sort_order IS '明細順序，從 0 開始（不是欄位）';
COMMENT ON COLUMN purchase_return_lines.goods_receipt_line_id IS '退回的進貨明細（relation → goods_receipt_line，預設隱藏）';
COMMENT ON COLUMN purchase_return_lines.product_id IS '= 進貨明細的商品，系統維護';
COMMENT ON COLUMN purchase_return_lines.lot_no IS '= 進貨明細的批號，系統維護';
COMMENT ON COLUMN purchase_return_lines.unit_price IS '= 進貨明細的單價，系統維護';
COMMENT ON COLUMN purchase_return_lines.unit_cost_twd IS '過帳時寫入批號的成本';
COMMENT ON COLUMN purchase_return_lines.lot_id IS '= 進貨明細的批號（不是欄位）；service 為 NULL';

CREATE INDEX idx_purchase_return_lines_purchase_return_id
    ON purchase_return_lines (purchase_return_id, sort_order, id) WHERE deleted_at IS NULL;
CREATE INDEX idx_purchase_return_lines_goods_receipt_line_id ON purchase_return_lines (goods_receipt_line_id);
CREATE INDEX idx_purchase_return_lines_product_id ON purchase_return_lines (product_id);
CREATE INDEX idx_purchase_return_lines_lot_id ON purchase_return_lines (lot_id);
CREATE INDEX idx_purchase_return_lines_custom_data ON purchase_return_lines USING GIN (custom_data);

CREATE TRIGGER trg_purchase_return_lines_set_updated_at
    BEFORE UPDATE ON purchase_return_lines
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- 庫存的來源類型（§8.10）
-- ============================================================================

ALTER TABLE inventory_lots DROP CONSTRAINT inventory_lots_source_type_check;
ALTER TABLE inventory_lots ADD CONSTRAINT inventory_lots_source_type_check
    CHECK (source_type IN ('stock_adjustment_line', 'goods_receipt_line'));

ALTER TABLE lot_allocations DROP CONSTRAINT lot_allocations_source_type_check;
ALTER TABLE lot_allocations ADD CONSTRAINT lot_allocations_source_type_check
    CHECK (source_type IN ('stock_adjustment_line', 'purchase_return_line'));

ALTER TABLE inventory_transactions DROP CONSTRAINT inventory_transactions_ref_doc_type_check;
ALTER TABLE inventory_transactions ADD CONSTRAINT inventory_transactions_ref_doc_type_check
    CHECK (ref_doc_type IN ('stock_adjustment', 'goods_receipt', 'purchase_return'));

-- ============================================================================
-- 系統資料類別（§8.8「資料類別」）
-- ============================================================================

INSERT INTO entity_definitions
    (id, key, label, kind, is_system, storage, table_name, parent_entity_id,
     numbering_pattern, title_template, sort_order)
VALUES
    ('01a1f4c0-5e10-7000-961c-f268733a2a58', 'purchase_order', '採購單', 'document', true, 'table',
     'purchase_orders', NULL, 'PO{yyyyMM}-{0000}', '{record_no} {supplier_id.name}', 110),
    ('01a1f4c0-5e10-7001-a354-dccb85b65acf', 'purchase_order_line', '採購明細', 'detail', true, 'table',
     'purchase_order_lines', '01a1f4c0-5e10-7000-961c-f268733a2a58', NULL, '{product_id.code} {product_id.name}', 0),
    ('01a1f4c0-5e10-7002-a3f6-25702f618e4d', 'goods_receipt', '進貨單', 'document', true, 'table',
     'goods_receipts', NULL, 'GR{yyyyMM}-{0000}', '{record_no} {supplier_id.name}', 120),
    ('01a1f4c0-5e10-7003-b2b2-bf5f574e1236', 'goods_receipt_line', '進貨明細', 'detail', true, 'table',
     'goods_receipt_lines', '01a1f4c0-5e10-7002-a3f6-25702f618e4d', NULL, '{product_id.code} {product_id.name} {lot_no}', 0),
    ('01a1f4c0-5e10-7004-9f63-b2ea144e4b01', 'purchase_return', '進貨退回', 'document', true, 'table',
     'purchase_returns', NULL, 'PR{yyyyMM}-{0000}', '{record_no} {supplier_id.name}', 130),
    ('01a1f4c0-5e10-7005-bd93-d9d2e7f08025', 'purchase_return_line', '進貨退回明細', 'detail', true, 'table',
     'purchase_return_lines', '01a1f4c0-5e10-7004-9f63-b2ea144e4b01', NULL, '{product_id.code} {product_id.name} {lot_no}', 0);

-- ============================================================================
-- 系統欄位（sort_order 依表中順序 10、20、30…；唯讀的為系統維護的欄位，§7.1）
-- ============================================================================

-- purchase_order
INSERT INTO field_definitions
    (id, entity_id, field_key, label, data_type, is_system, storage, column_name,
     is_required, is_visible, is_readonly, default_value, validation, options,
     decimal_places, relation_entity_id, ui_widget, sort_order)
VALUES
    ('01a1f4c0-5e10-7006-8227-395aabad1031', '01a1f4c0-5e10-7000-961c-f268733a2a58', 'supplier_id', '廠商', 'relation', true, 'column', 'supplier_id',
     true, true, false, NULL, NULL, NULL,
     NULL, '01a1e516-3406-71fc-8cf2-9ce47caf0e68', NULL, 10),
    ('01a1f4c0-5e10-7007-9e16-0548534313b4', '01a1f4c0-5e10-7000-961c-f268733a2a58', 'order_date', '採購日期', 'date', true, 'column', 'order_date',
     true, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 20),
    ('01a1f4c0-5e10-7008-ae13-0dba3be4dcc2', '01a1f4c0-5e10-7000-961c-f268733a2a58', 'expected_date', '預計到貨日', 'date', true, 'column', 'expected_date',
     false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 30),
    ('01a1f4c0-5e10-7009-9eee-99a04215dfcb', '01a1f4c0-5e10-7000-961c-f268733a2a58', 'currency_code', '幣別', 'text', true, 'column', 'currency_code',
     true, true, false, NULL, NULL, NULL,
     NULL, NULL, 'currency', 40),
    ('01a1f4c0-5e10-700a-bae5-a7e0222d76f1', '01a1f4c0-5e10-7000-961c-f268733a2a58', 'exchange_rate', '匯率', 'decimal', true, 'column', 'exchange_rate',
     false, true, false, NULL, '{"min":"0"}'::jsonb, NULL,
     6, NULL, NULL, 50),
    ('01a1f4c0-5e10-700b-a667-8661a9f6d0b0', '01a1f4c0-5e10-7000-961c-f268733a2a58', 'tax_type', '稅別', 'select', true, 'column', 'tax_type',
     true, true, false, NULL, NULL, '[{"value":"taxable","label":"應稅"},{"value":"zero_rated","label":"零稅率"},{"value":"exempt","label":"免稅"}]'::jsonb,
     NULL, NULL, NULL, 60),
    ('01a1f4c0-5e10-700c-8b6e-877b643ad210', '01a1f4c0-5e10-7000-961c-f268733a2a58', 'status', '狀態', 'select', true, 'column', 'status',
     false, true, true, NULL, NULL, '[{"value":"draft","label":"草稿"},{"value":"confirmed","label":"已確認"},{"value":"partially_received","label":"部分進貨"},{"value":"closed","label":"已結案"},{"value":"void","label":"已作廢"}]'::jsonb,
     NULL, NULL, NULL, 70),
    ('01a1f4c0-5e10-700d-8979-d520e768e589', '01a1f4c0-5e10-7000-961c-f268733a2a58', 'subtotal', '未稅金額', 'money', true, 'column', 'subtotal',
     false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 80),
    ('01a1f4c0-5e10-700e-be29-c98309adfea9', '01a1f4c0-5e10-7000-961c-f268733a2a58', 'tax_amount', '稅額', 'money', true, 'column', 'tax_amount',
     false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 90),
    ('01a1f4c0-5e10-700f-8078-f6eaad6cef3a', '01a1f4c0-5e10-7000-961c-f268733a2a58', 'total', '總計', 'money', true, 'column', 'total',
     false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 100),
    ('01a1f4c0-5e10-7010-b3d6-1d3009037ac3', '01a1f4c0-5e10-7000-961c-f268733a2a58', 'total_twd', '台幣總計', 'money', true, 'column', 'total_twd',
     false, true, true, NULL, NULL, NULL,
     2, NULL, NULL, 110),
    ('01a1f4c0-5e10-7011-b021-855455dfe1d0', '01a1f4c0-5e10-7000-961c-f268733a2a58', 'notes', '備註', 'long_text', true, 'column', 'notes',
     false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 120);

-- purchase_order_line
INSERT INTO field_definitions
    (id, entity_id, field_key, label, data_type, is_system, storage, column_name,
     is_required, is_visible, is_readonly, default_value, validation, options,
     decimal_places, relation_entity_id, ui_widget, sort_order)
VALUES
    ('01a1f4c0-5e10-7012-93e1-d8074f64f196', '01a1f4c0-5e10-7001-a354-dccb85b65acf', 'product_id', '商品', 'relation', true, 'column', 'product_id',
     true, true, false, NULL, NULL, NULL,
     NULL, '01a1e516-3401-7432-acb3-9cc3ac6a7db9', NULL, 10),
    ('01a1f4c0-5e10-7013-81d2-d842c03a1de7', '01a1f4c0-5e10-7001-a354-dccb85b65acf', 'qty', '數量', 'decimal', true, 'column', 'qty',
     true, true, false, NULL, '{"min":"0"}'::jsonb, NULL,
     4, NULL, NULL, 20),
    ('01a1f4c0-5e10-7014-a273-332829220c27', '01a1f4c0-5e10-7001-a354-dccb85b65acf', 'unit_price', '單價', 'money', true, 'column', 'unit_price',
     true, true, false, NULL, '{"min":"0"}'::jsonb, NULL,
     4, NULL, NULL, 30),
    ('01a1f4c0-5e10-7015-818c-59cee6995a17', '01a1f4c0-5e10-7001-a354-dccb85b65acf', 'amount', '金額', 'money', true, 'column', 'amount',
     false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 40),
    ('01a1f4c0-5e10-7016-a485-c9baa00dc814', '01a1f4c0-5e10-7001-a354-dccb85b65acf', 'received_qty', '已進貨數量', 'decimal', true, 'column', 'received_qty',
     false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 50),
    ('01a1f4c0-5e10-7017-a115-44ef9497fe37', '01a1f4c0-5e10-7001-a354-dccb85b65acf', 'notes', '備註', 'long_text', true, 'column', 'notes',
     false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 60);

-- goods_receipt
INSERT INTO field_definitions
    (id, entity_id, field_key, label, data_type, is_system, storage, column_name,
     is_required, is_visible, is_readonly, default_value, validation, options,
     decimal_places, relation_entity_id, ui_widget, sort_order)
VALUES
    ('01a1f4c0-5e10-7018-b9c0-60c9be3ac100', '01a1f4c0-5e10-7002-a3f6-25702f618e4d', 'supplier_id', '廠商', 'relation', true, 'column', 'supplier_id',
     true, true, false, NULL, NULL, NULL,
     NULL, '01a1e516-3406-71fc-8cf2-9ce47caf0e68', NULL, 10),
    ('01a1f4c0-5e10-7019-a3b8-ca6b52a0deda', '01a1f4c0-5e10-7002-a3f6-25702f618e4d', 'purchase_order_id', '採購單', 'relation', true, 'column', 'purchase_order_id',
     false, true, false, NULL, NULL, NULL,
     NULL, '01a1f4c0-5e10-7000-961c-f268733a2a58', NULL, 20),
    ('01a1f4c0-5e10-701a-b61b-6f98d6345834', '01a1f4c0-5e10-7002-a3f6-25702f618e4d', 'receipt_date', '進貨日期', 'date', true, 'column', 'receipt_date',
     true, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 30),
    ('01a1f4c0-5e10-701b-a6bd-8b44630f0489', '01a1f4c0-5e10-7002-a3f6-25702f618e4d', 'currency_code', '幣別', 'text', true, 'column', 'currency_code',
     true, true, false, NULL, NULL, NULL,
     NULL, NULL, 'currency', 40),
    ('01a1f4c0-5e10-701c-bb7e-7e33d60bdb3b', '01a1f4c0-5e10-7002-a3f6-25702f618e4d', 'exchange_rate', '匯率', 'decimal', true, 'column', 'exchange_rate',
     false, true, false, NULL, '{"min":"0"}'::jsonb, NULL,
     6, NULL, NULL, 50),
    ('01a1f4c0-5e10-701d-865c-4e645b53098e', '01a1f4c0-5e10-7002-a3f6-25702f618e4d', 'tax_type', '稅別', 'select', true, 'column', 'tax_type',
     true, true, false, NULL, NULL, '[{"value":"taxable","label":"應稅"},{"value":"zero_rated","label":"零稅率"},{"value":"exempt","label":"免稅"}]'::jsonb,
     NULL, NULL, NULL, 60),
    ('01a1f4c0-5e10-701e-9be8-ca5fa2146533', '01a1f4c0-5e10-7002-a3f6-25702f618e4d', 'status', '狀態', 'select', true, 'column', 'status',
     false, true, true, NULL, NULL, '[{"value":"draft","label":"草稿"},{"value":"posted","label":"已過帳"},{"value":"void","label":"已作廢"}]'::jsonb,
     NULL, NULL, NULL, 70),
    ('01a1f4c0-5e10-701f-99af-d8b4863df4df', '01a1f4c0-5e10-7002-a3f6-25702f618e4d', 'subtotal', '未稅金額', 'money', true, 'column', 'subtotal',
     false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 80),
    ('01a1f4c0-5e10-7020-8375-730f12cec777', '01a1f4c0-5e10-7002-a3f6-25702f618e4d', 'tax_amount', '稅額', 'money', true, 'column', 'tax_amount',
     false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 90),
    ('01a1f4c0-5e10-7021-b1c1-8aeb45d999ae', '01a1f4c0-5e10-7002-a3f6-25702f618e4d', 'total', '總計', 'money', true, 'column', 'total',
     false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 100),
    ('01a1f4c0-5e10-7022-82a0-255cf9ce26ef', '01a1f4c0-5e10-7002-a3f6-25702f618e4d', 'total_twd', '台幣總計', 'money', true, 'column', 'total_twd',
     false, true, true, NULL, NULL, NULL,
     2, NULL, NULL, 110),
    ('01a1f4c0-5e10-7023-a22b-74cd8905e39d', '01a1f4c0-5e10-7002-a3f6-25702f618e4d', 'notes', '備註', 'long_text', true, 'column', 'notes',
     false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 120);

-- goods_receipt_line
INSERT INTO field_definitions
    (id, entity_id, field_key, label, data_type, is_system, storage, column_name,
     is_required, is_visible, is_readonly, default_value, validation, options,
     decimal_places, relation_entity_id, ui_widget, sort_order)
VALUES
    ('01a1f4c0-5e10-7024-b504-36b56aa39c0c', '01a1f4c0-5e10-7003-b2b2-bf5f574e1236', 'purchase_order_line_id', '採購明細', 'relation', true, 'column', 'purchase_order_line_id',
     false, false, false, NULL, NULL, NULL,
     NULL, '01a1f4c0-5e10-7001-a354-dccb85b65acf', NULL, 10),
    ('01a1f4c0-5e10-7025-b890-67c86147b6ad', '01a1f4c0-5e10-7003-b2b2-bf5f574e1236', 'product_id', '商品', 'relation', true, 'column', 'product_id',
     true, true, false, NULL, NULL, NULL,
     NULL, '01a1e516-3401-7432-acb3-9cc3ac6a7db9', NULL, 20),
    ('01a1f4c0-5e10-7026-a83b-016c363c4cc2', '01a1f4c0-5e10-7003-b2b2-bf5f574e1236', 'qty', '數量', 'decimal', true, 'column', 'qty',
     true, true, false, NULL, '{"min":"0"}'::jsonb, NULL,
     4, NULL, NULL, 30),
    ('01a1f4c0-5e10-7027-bb49-6741f0177f47', '01a1f4c0-5e10-7003-b2b2-bf5f574e1236', 'unit_price', '單價', 'money', true, 'column', 'unit_price',
     true, true, false, NULL, '{"min":"0"}'::jsonb, NULL,
     4, NULL, NULL, 40),
    ('01a1f4c0-5e10-7028-81d7-5928f1db83e1', '01a1f4c0-5e10-7003-b2b2-bf5f574e1236', 'amount', '金額', 'money', true, 'column', 'amount',
     false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 50),
    ('01a1f4c0-5e10-7029-b810-d8e96a945d4d', '01a1f4c0-5e10-7003-b2b2-bf5f574e1236', 'lot_no', '批號', 'text', true, 'column', 'lot_no',
     false, true, false, NULL, '{"max_length":50}'::jsonb, NULL,
     NULL, NULL, NULL, 60),
    ('01a1f4c0-5e10-702a-9a5e-8dd8642cb871', '01a1f4c0-5e10-7003-b2b2-bf5f574e1236', 'unit_cost_twd', '台幣單位成本', 'money', true, 'column', 'unit_cost_twd',
     false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 70),
    ('01a1f4c0-5e10-702b-b6b9-9845a009d7da', '01a1f4c0-5e10-7003-b2b2-bf5f574e1236', 'notes', '備註', 'long_text', true, 'column', 'notes',
     false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 80);

-- purchase_return
INSERT INTO field_definitions
    (id, entity_id, field_key, label, data_type, is_system, storage, column_name,
     is_required, is_visible, is_readonly, default_value, validation, options,
     decimal_places, relation_entity_id, ui_widget, sort_order)
VALUES
    ('01a1f4c0-5e10-702c-b191-a54916b78553', '01a1f4c0-5e10-7004-9f63-b2ea144e4b01', 'goods_receipt_id', '進貨單', 'relation', true, 'column', 'goods_receipt_id',
     true, true, false, NULL, NULL, NULL,
     NULL, '01a1f4c0-5e10-7002-a3f6-25702f618e4d', NULL, 10),
    ('01a1f4c0-5e10-702d-b280-b4a033f9e4a6', '01a1f4c0-5e10-7004-9f63-b2ea144e4b01', 'supplier_id', '廠商', 'relation', true, 'column', 'supplier_id',
     false, true, true, NULL, NULL, NULL,
     NULL, '01a1e516-3406-71fc-8cf2-9ce47caf0e68', NULL, 20),
    ('01a1f4c0-5e10-702e-be00-d49a2029fb9b', '01a1f4c0-5e10-7004-9f63-b2ea144e4b01', 'return_date', '退回日期', 'date', true, 'column', 'return_date',
     true, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 30),
    ('01a1f4c0-5e10-702f-912f-456fce63b8a1', '01a1f4c0-5e10-7004-9f63-b2ea144e4b01', 'currency_code', '幣別', 'text', true, 'column', 'currency_code',
     false, true, true, NULL, NULL, NULL,
     NULL, NULL, 'currency', 40),
    ('01a1f4c0-5e10-7030-8aab-c4cb0a5792b3', '01a1f4c0-5e10-7004-9f63-b2ea144e4b01', 'exchange_rate', '匯率', 'decimal', true, 'column', 'exchange_rate',
     false, true, true, NULL, NULL, NULL,
     6, NULL, NULL, 50),
    ('01a1f4c0-5e10-7031-bf7c-1298f4327909', '01a1f4c0-5e10-7004-9f63-b2ea144e4b01', 'tax_type', '稅別', 'select', true, 'column', 'tax_type',
     false, true, true, NULL, NULL, '[{"value":"taxable","label":"應稅"},{"value":"zero_rated","label":"零稅率"},{"value":"exempt","label":"免稅"}]'::jsonb,
     NULL, NULL, NULL, 60),
    ('01a1f4c0-5e10-7032-a56b-7bec524ed0a4', '01a1f4c0-5e10-7004-9f63-b2ea144e4b01', 'status', '狀態', 'select', true, 'column', 'status',
     false, true, true, NULL, NULL, '[{"value":"draft","label":"草稿"},{"value":"posted","label":"已過帳"},{"value":"void","label":"已作廢"}]'::jsonb,
     NULL, NULL, NULL, 70),
    ('01a1f4c0-5e10-7033-b17b-57f3dd8c8bdc', '01a1f4c0-5e10-7004-9f63-b2ea144e4b01', 'subtotal', '未稅金額', 'money', true, 'column', 'subtotal',
     false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 80),
    ('01a1f4c0-5e10-7034-9ff6-fc258fd66746', '01a1f4c0-5e10-7004-9f63-b2ea144e4b01', 'tax_amount', '稅額', 'money', true, 'column', 'tax_amount',
     false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 90),
    ('01a1f4c0-5e10-7035-a81f-3dac31561237', '01a1f4c0-5e10-7004-9f63-b2ea144e4b01', 'total', '總計', 'money', true, 'column', 'total',
     false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 100),
    ('01a1f4c0-5e10-7036-8149-1c55862285b6', '01a1f4c0-5e10-7004-9f63-b2ea144e4b01', 'total_twd', '台幣總計', 'money', true, 'column', 'total_twd',
     false, true, true, NULL, NULL, NULL,
     2, NULL, NULL, 110),
    ('01a1f4c0-5e10-7037-974e-568f89edb024', '01a1f4c0-5e10-7004-9f63-b2ea144e4b01', 'reason', '退回原因', 'text', true, 'column', 'reason',
     false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 120),
    ('01a1f4c0-5e10-7038-97c3-db379d834d6f', '01a1f4c0-5e10-7004-9f63-b2ea144e4b01', 'notes', '備註', 'long_text', true, 'column', 'notes',
     false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 130);

-- purchase_return_line
INSERT INTO field_definitions
    (id, entity_id, field_key, label, data_type, is_system, storage, column_name,
     is_required, is_visible, is_readonly, default_value, validation, options,
     decimal_places, relation_entity_id, ui_widget, sort_order)
VALUES
    ('01a1f4c0-5e10-7039-b9e8-ebcaf7a46002', '01a1f4c0-5e10-7005-bd93-d9d2e7f08025', 'goods_receipt_line_id', '進貨明細', 'relation', true, 'column', 'goods_receipt_line_id',
     true, false, false, NULL, NULL, NULL,
     NULL, '01a1f4c0-5e10-7003-b2b2-bf5f574e1236', NULL, 10),
    ('01a1f4c0-5e10-703a-836e-6880950d4eb4', '01a1f4c0-5e10-7005-bd93-d9d2e7f08025', 'product_id', '商品', 'relation', true, 'column', 'product_id',
     false, true, true, NULL, NULL, NULL,
     NULL, '01a1e516-3401-7432-acb3-9cc3ac6a7db9', NULL, 20),
    ('01a1f4c0-5e10-703b-b568-da555b2a909b', '01a1f4c0-5e10-7005-bd93-d9d2e7f08025', 'lot_no', '批號', 'text', true, 'column', 'lot_no',
     false, true, true, NULL, NULL, NULL,
     NULL, NULL, NULL, 30),
    ('01a1f4c0-5e10-703c-9165-612a71d8c1ab', '01a1f4c0-5e10-7005-bd93-d9d2e7f08025', 'qty', '數量', 'decimal', true, 'column', 'qty',
     true, true, false, NULL, '{"min":"0"}'::jsonb, NULL,
     4, NULL, NULL, 40),
    ('01a1f4c0-5e10-703d-91d2-8af4a1401373', '01a1f4c0-5e10-7005-bd93-d9d2e7f08025', 'unit_price', '單價', 'money', true, 'column', 'unit_price',
     false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 50),
    ('01a1f4c0-5e10-703e-9790-630b315b6b4d', '01a1f4c0-5e10-7005-bd93-d9d2e7f08025', 'amount', '金額', 'money', true, 'column', 'amount',
     false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 60),
    ('01a1f4c0-5e10-703f-af45-f42ef2a6e6e1', '01a1f4c0-5e10-7005-bd93-d9d2e7f08025', 'unit_cost_twd', '台幣單位成本', 'money', true, 'column', 'unit_cost_twd',
     false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 70),
    ('01a1f4c0-5e10-7040-86cd-584974931e58', '01a1f4c0-5e10-7005-bd93-d9d2e7f08025', 'notes', '備註', 'long_text', true, 'column', 'notes',
     false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 80);

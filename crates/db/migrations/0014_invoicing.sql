-- 0014_invoicing：M5 發票模組的資料表與資料類別（#183，docs/architecture.md §8.11）。
--
-- 建立四張系統單據（銷項發票、銷貨折讓、進項憑證、進貨折讓）與其明細子表，依 §8.6「系統資料表的共同規則」
-- 與 §7.1「系統單據」：系統欄位 → custom_data → formula_errors → 共通欄位；單號 record_no 與開立、作廢的時間與人員
-- 不是欄位。金額一律是台幣。
--
-- 關聯的出貨單、進貨單以明細子表記錄（sales_invoice_shipment、purchase_invoice_receipt），與其他明細一樣整組取代。
-- 「一張出貨單只在一張有效發票中」跨越主表的狀態，無法以索引表達，由 hammer_core 在開立時鎖定出貨單後檢查（#184）。
--
-- 發票號碼的唯一規則（§8.11「發票號碼」）：
-- - 銷項：全域唯一、含已作廢（WHERE status <> 'draft'）
-- - 進項：同一廠商 + 號碼唯一、不含已作廢（WHERE status = 'posted'）
-- 草稿不受限制，開立時才檢查。
--
-- 系統維護的欄位（status、期別、金額合計、作廢日期與原因、折讓的客戶與稅別等）是唯讀的系統欄位，
-- 由 hammer_core 在存檔、開立、作廢時寫入。
--
-- 資料類別、欄位定義的 id 為固定的 uuid v7，方便跨環境比對。
-- 欄位定義必須與 architecture.md §8.11 一致，由 crates/db/tests/invoicing.rs 檢查。

-- ============================================================================
-- sales_invoices（銷項發票，系統單據 sales_invoice）
-- ============================================================================

CREATE TABLE sales_invoices (
    id                 UUID PRIMARY KEY,
    record_no          TEXT NOT NULL,
    invoice_type       TEXT NOT NULL DEFAULT 'triplicate' CHECK (invoice_type IN ('triplicate', 'duplicate')),
    invoice_no         TEXT NOT NULL,
    invoice_date       DATE NOT NULL,
    tax_period         TEXT NOT NULL,
    customer_id        UUID NOT NULL REFERENCES customers(id),
    buyer_tax_id       TEXT,
    buyer_name         TEXT,
    buyer_address      TEXT,
    tax_type           TEXT NOT NULL REFERENCES tax_rates(code),
    tax_amount_manual  NUMERIC(18,4) CHECK (tax_amount_manual >= 0),
    status             TEXT NOT NULL DEFAULT 'draft'
                       CHECK (status IN ('draft', 'posted', 'void')),
    subtotal           NUMERIC(18,4) NOT NULL DEFAULT 0,
    tax_amount         NUMERIC(18,4) NOT NULL DEFAULT 0,
    total              NUMERIC(18,4) NOT NULL DEFAULT 0,
    void_date          DATE,
    void_reason        TEXT,
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

    CONSTRAINT sales_invoices_posted_status_check
        CHECK ((status = 'draft') = (posted_at IS NULL)),
    CONSTRAINT sales_invoices_voided_status_check
        CHECK ((status = 'void') = (voided_at IS NOT NULL)),
    CONSTRAINT sales_invoices_posted_by_check
        CHECK ((posted_at IS NULL) = (posted_by IS NULL)),
    CONSTRAINT sales_invoices_voided_by_check
        CHECK ((voided_at IS NULL) = (voided_by IS NULL)),
    CONSTRAINT sales_invoices_void_date_check
        CHECK ((status = 'void') = (void_date IS NOT NULL)),
    CONSTRAINT sales_invoices_void_reason_check
        CHECK ((void_date IS NULL) = (void_reason IS NULL))
);

COMMENT ON TABLE sales_invoices IS '銷項發票（§8.11，系統單據 sales_invoice）';
COMMENT ON COLUMN sales_invoices.record_no IS '系統單號（不是欄位），依 numbering_pattern 取號；與發票號碼 invoice_no 分開';
COMMENT ON COLUMN sales_invoices.invoice_type IS 'triplicate（三聯式，單價未稅）/ duplicate（二聯式，單價含稅）';
COMMENT ON COLUMN sales_invoices.invoice_no IS '發票號碼：2 碼英文 + 8 碼數字（大寫）；開立後全域唯一，含已作廢';
COMMENT ON COLUMN sales_invoices.tax_period IS '期別：發票日期的年 + 該期第一個月（奇數月），例如 202611；系統維護';
COMMENT ON COLUMN sales_invoices.buyer_tax_id IS '買受人統一編號（客戶資料的快照）；三聯式開立時必填';
COMMENT ON COLUMN sales_invoices.tax_amount_manual IS '手動修正的稅額；NULL = 依系統計算（§8.11「金額與稅額」）';
COMMENT ON COLUMN sales_invoices.status IS 'draft / posted（已開立）/ void，系統維護';
COMMENT ON COLUMN sales_invoices.void_date IS '作廢日期（作廢端點寫入）；可與發票不同期';

CREATE UNIQUE INDEX uq_sales_invoices_record_no ON sales_invoices (record_no);
CREATE UNIQUE INDEX uq_sales_invoices_invoice_no ON sales_invoices (invoice_no) WHERE status <> 'draft';
CREATE INDEX idx_sales_invoices_invoice_no ON sales_invoices (lower(invoice_no));
CREATE INDEX idx_sales_invoices_buyer_tax_id ON sales_invoices (lower(buyer_tax_id));
CREATE INDEX idx_sales_invoices_invoice_date ON sales_invoices (invoice_date, id);
CREATE INDEX idx_sales_invoices_status ON sales_invoices (status);
CREATE INDEX idx_sales_invoices_customer_id ON sales_invoices (customer_id);
CREATE INDEX idx_sales_invoices_custom_data ON sales_invoices USING GIN (custom_data);

CREATE TRIGGER trg_sales_invoices_set_updated_at
    BEFORE UPDATE ON sales_invoices
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- sales_invoice_lines（明細子表 sales_invoice_line）
-- ============================================================================

CREATE TABLE sales_invoice_lines (
    id                 UUID PRIMARY KEY,
    sales_invoice_id   UUID NOT NULL REFERENCES sales_invoices(id),
    sort_order         INTEGER NOT NULL DEFAULT 0,
    product_id         UUID REFERENCES products(id),
    description        TEXT NOT NULL,
    qty                NUMERIC(18,4) NOT NULL CHECK (qty > 0),
    unit_price         NUMERIC(18,4) NOT NULL CHECK (unit_price >= 0),
    amount             NUMERIC(18,4) NOT NULL DEFAULT 0,
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

COMMENT ON TABLE sales_invoice_lines IS '發票明細（§8.11，明細子表 sales_invoice_line）';
COMMENT ON COLUMN sales_invoice_lines.sales_invoice_id IS '所屬發票（不是欄位）';
COMMENT ON COLUMN sales_invoice_lines.sort_order IS '明細順序，從 0 開始（不是欄位）';
COMMENT ON COLUMN sales_invoice_lines.product_id IS '商品（參考用，可以不填）';
COMMENT ON COLUMN sales_invoice_lines.description IS '發票上印的品名';
COMMENT ON COLUMN sales_invoice_lines.unit_price IS '單價：三聯式未稅、二聯式含稅';
COMMENT ON COLUMN sales_invoice_lines.amount IS 'round(qty × unit_price, 台幣小數位數)，系統維護';

CREATE INDEX idx_sales_invoice_lines_sales_invoice_id
    ON sales_invoice_lines (sales_invoice_id, sort_order, id) WHERE deleted_at IS NULL;
CREATE INDEX idx_sales_invoice_lines_product_id ON sales_invoice_lines (product_id);
CREATE INDEX idx_sales_invoice_lines_custom_data ON sales_invoice_lines USING GIN (custom_data);

CREATE TRIGGER trg_sales_invoice_lines_set_updated_at
    BEFORE UPDATE ON sales_invoice_lines
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- sales_invoice_shipments（明細子表 sales_invoice_shipment：發票關聯的出貨單）
-- ============================================================================

CREATE TABLE sales_invoice_shipments (
    id                 UUID PRIMARY KEY,
    sales_invoice_id   UUID NOT NULL REFERENCES sales_invoices(id),
    sort_order         INTEGER NOT NULL DEFAULT 0,
    shipment_id        UUID NOT NULL REFERENCES shipments(id),
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

COMMENT ON TABLE sales_invoice_shipments IS '發票關聯的出貨單（§8.11，明細子表 sales_invoice_shipment）';
COMMENT ON COLUMN sales_invoice_shipments.sales_invoice_id IS '所屬發票（不是欄位）';
COMMENT ON COLUMN sales_invoice_shipments.shipment_id IS '已過帳、同客戶的出貨單；一張出貨單只在一張有效發票中（hammer_core 檢查）';

CREATE INDEX idx_sales_invoice_shipments_sales_invoice_id
    ON sales_invoice_shipments (sales_invoice_id, sort_order, id) WHERE deleted_at IS NULL;
CREATE INDEX idx_sales_invoice_shipments_shipment_id ON sales_invoice_shipments (shipment_id);
CREATE INDEX idx_sales_invoice_shipments_custom_data ON sales_invoice_shipments USING GIN (custom_data);

CREATE TRIGGER trg_sales_invoice_shipments_set_updated_at
    BEFORE UPDATE ON sales_invoice_shipments
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- sales_allowances（銷貨折讓，系統單據 sales_allowance）
-- ============================================================================

CREATE TABLE sales_allowances (
    id                 UUID PRIMARY KEY,
    record_no          TEXT NOT NULL,
    sales_invoice_id   UUID NOT NULL REFERENCES sales_invoices(id),
    customer_id        UUID NOT NULL REFERENCES customers(id),
    allowance_date     DATE NOT NULL,
    tax_period         TEXT NOT NULL,
    tax_type           TEXT NOT NULL REFERENCES tax_rates(code),
    sales_return_id    UUID REFERENCES sales_returns(id),
    tax_amount_manual  NUMERIC(18,4) CHECK (tax_amount_manual >= 0),
    status             TEXT NOT NULL DEFAULT 'draft'
                       CHECK (status IN ('draft', 'posted', 'void')),
    subtotal           NUMERIC(18,4) NOT NULL DEFAULT 0,
    tax_amount         NUMERIC(18,4) NOT NULL DEFAULT 0,
    total              NUMERIC(18,4) NOT NULL DEFAULT 0,
    reason             TEXT,
    void_date          DATE,
    void_reason        TEXT,
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

    CONSTRAINT sales_allowances_posted_status_check
        CHECK ((status = 'draft') = (posted_at IS NULL)),
    CONSTRAINT sales_allowances_voided_status_check
        CHECK ((status = 'void') = (voided_at IS NOT NULL)),
    CONSTRAINT sales_allowances_posted_by_check
        CHECK ((posted_at IS NULL) = (posted_by IS NULL)),
    CONSTRAINT sales_allowances_voided_by_check
        CHECK ((voided_at IS NULL) = (voided_by IS NULL)),
    CONSTRAINT sales_allowances_void_date_check
        CHECK ((status = 'void') = (void_date IS NOT NULL)),
    CONSTRAINT sales_allowances_void_reason_check
        CHECK ((void_date IS NULL) = (void_reason IS NULL))
);

COMMENT ON TABLE sales_allowances IS '銷貨折讓證明單（§8.11，系統單據 sales_allowance）';
COMMENT ON COLUMN sales_allowances.record_no IS '系統單號（不是欄位）；折讓證明單以單號識別';
COMMENT ON COLUMN sales_allowances.sales_invoice_id IS '原發票（必須已開立）；建立後不可更換';
COMMENT ON COLUMN sales_allowances.customer_id IS '= 發票的客戶，系統維護';
COMMENT ON COLUMN sales_allowances.tax_type IS '= 發票的稅別，系統維護';
COMMENT ON COLUMN sales_allowances.sales_return_id IS '關聯的銷貨退回（可不填）；一張退回只在一張有效折讓中（hammer_core 檢查）';

CREATE UNIQUE INDEX uq_sales_allowances_record_no ON sales_allowances (record_no);
CREATE INDEX idx_sales_allowances_allowance_date ON sales_allowances (allowance_date, id);
CREATE INDEX idx_sales_allowances_status ON sales_allowances (status);
CREATE INDEX idx_sales_allowances_customer_id ON sales_allowances (customer_id);
CREATE INDEX idx_sales_allowances_sales_invoice_id ON sales_allowances (sales_invoice_id);
CREATE INDEX idx_sales_allowances_sales_return_id ON sales_allowances (sales_return_id);
CREATE INDEX idx_sales_allowances_custom_data ON sales_allowances USING GIN (custom_data);

CREATE TRIGGER trg_sales_allowances_set_updated_at
    BEFORE UPDATE ON sales_allowances
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- sales_allowance_lines（明細子表 sales_allowance_line）
-- ============================================================================

CREATE TABLE sales_allowance_lines (
    id                  UUID PRIMARY KEY,
    sales_allowance_id  UUID NOT NULL REFERENCES sales_allowances(id),
    sort_order          INTEGER NOT NULL DEFAULT 0,
    product_id          UUID REFERENCES products(id),
    description         TEXT NOT NULL,
    qty                 NUMERIC(18,4) NOT NULL CHECK (qty > 0),
    unit_price          NUMERIC(18,4) NOT NULL CHECK (unit_price >= 0),
    amount              NUMERIC(18,4) NOT NULL DEFAULT 0,
    notes               TEXT,
    custom_data         JSONB NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(custom_data) = 'object'),
    formula_errors      JSONB CHECK (formula_errors IS NULL OR jsonb_typeof(formula_errors) = 'object'),
    -- 共通欄位（§8.1）
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by          UUID REFERENCES users(id),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by          UUID REFERENCES users(id),
    deleted_at          TIMESTAMPTZ,
    version             INTEGER NOT NULL DEFAULT 1
);

COMMENT ON TABLE sales_allowance_lines IS '折讓明細（§8.11，明細子表 sales_allowance_line）';
COMMENT ON COLUMN sales_allowance_lines.sales_allowance_id IS '所屬折讓（不是欄位）';
COMMENT ON COLUMN sales_allowance_lines.sort_order IS '明細順序，從 0 開始（不是欄位）';
COMMENT ON COLUMN sales_allowance_lines.unit_price IS '單價，一律未稅';
COMMENT ON COLUMN sales_allowance_lines.amount IS 'round(qty × unit_price, 台幣小數位數)，系統維護';

CREATE INDEX idx_sales_allowance_lines_sales_allowance_id
    ON sales_allowance_lines (sales_allowance_id, sort_order, id) WHERE deleted_at IS NULL;
CREATE INDEX idx_sales_allowance_lines_product_id ON sales_allowance_lines (product_id);
CREATE INDEX idx_sales_allowance_lines_custom_data ON sales_allowance_lines USING GIN (custom_data);

CREATE TRIGGER trg_sales_allowance_lines_set_updated_at
    BEFORE UPDATE ON sales_allowance_lines
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- purchase_invoices（進項憑證，系統單據 purchase_invoice）
-- ============================================================================

CREATE TABLE purchase_invoices (
    id                 UUID PRIMARY KEY,
    record_no          TEXT NOT NULL,
    doc_type           TEXT NOT NULL DEFAULT 'triplicate'
                       CHECK (doc_type IN ('triplicate', 'electronic', 'customs')),
    invoice_no         TEXT NOT NULL,
    invoice_date       DATE NOT NULL,
    tax_period         TEXT NOT NULL,
    supplier_id        UUID NOT NULL REFERENCES suppliers(id),
    seller_tax_id      TEXT,
    tax_type           TEXT NOT NULL REFERENCES tax_rates(code),
    subtotal           NUMERIC(18,4) NOT NULL CHECK (subtotal >= 0),
    tax_amount         NUMERIC(18,4) NOT NULL CHECK (tax_amount >= 0),
    total              NUMERIC(18,4) NOT NULL DEFAULT 0,
    deductible         BOOLEAN NOT NULL DEFAULT true,
    status             TEXT NOT NULL DEFAULT 'draft'
                       CHECK (status IN ('draft', 'posted', 'void')),
    void_date          DATE,
    void_reason        TEXT,
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

    CONSTRAINT purchase_invoices_posted_status_check
        CHECK ((status = 'draft') = (posted_at IS NULL)),
    CONSTRAINT purchase_invoices_voided_status_check
        CHECK ((status = 'void') = (voided_at IS NOT NULL)),
    CONSTRAINT purchase_invoices_posted_by_check
        CHECK ((posted_at IS NULL) = (posted_by IS NULL)),
    CONSTRAINT purchase_invoices_voided_by_check
        CHECK ((voided_at IS NULL) = (voided_by IS NULL)),
    CONSTRAINT purchase_invoices_void_date_check
        CHECK ((status = 'void') = (void_date IS NOT NULL)),
    CONSTRAINT purchase_invoices_void_reason_check
        CHECK ((void_date IS NULL) = (void_reason IS NULL))
);

COMMENT ON TABLE purchase_invoices IS '進項憑證（§8.11，系統單據 purchase_invoice）：沒有品名明細，只記表頭金額';
COMMENT ON COLUMN purchase_invoices.record_no IS '系統單號（不是欄位）；與憑證號碼 invoice_no 分開';
COMMENT ON COLUMN purchase_invoices.doc_type IS 'triplicate（三聯式發票）/ electronic（電子發票）/ customs（海關代徵營業稅繳納證）';
COMMENT ON COLUMN purchase_invoices.invoice_no IS '憑證號碼；登錄後同一廠商 + 號碼唯一，不含已作廢';
COMMENT ON COLUMN purchase_invoices.seller_tax_id IS '賣方統一編號（廠商資料的快照）';
COMMENT ON COLUMN purchase_invoices.subtotal IS '未稅金額（台幣，使用者輸入）；海關繳納證為完稅價格';
COMMENT ON COLUMN purchase_invoices.total IS 'subtotal + tax_amount，系統維護';
COMMENT ON COLUMN purchase_invoices.deductible IS '進項稅額是否可扣抵（紀錄用）';
COMMENT ON COLUMN purchase_invoices.status IS 'draft / posted（已登錄）/ void，系統維護';

CREATE UNIQUE INDEX uq_purchase_invoices_record_no ON purchase_invoices (record_no);
CREATE UNIQUE INDEX uq_purchase_invoices_invoice_no
    ON purchase_invoices (supplier_id, invoice_no) WHERE status = 'posted';
CREATE INDEX idx_purchase_invoices_invoice_no ON purchase_invoices (lower(invoice_no));
CREATE INDEX idx_purchase_invoices_invoice_date ON purchase_invoices (invoice_date, id);
CREATE INDEX idx_purchase_invoices_status ON purchase_invoices (status);
CREATE INDEX idx_purchase_invoices_supplier_id ON purchase_invoices (supplier_id);
CREATE INDEX idx_purchase_invoices_custom_data ON purchase_invoices USING GIN (custom_data);

CREATE TRIGGER trg_purchase_invoices_set_updated_at
    BEFORE UPDATE ON purchase_invoices
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- purchase_invoice_receipts（明細子表 purchase_invoice_receipt：憑證關聯的進貨單）
-- ============================================================================

CREATE TABLE purchase_invoice_receipts (
    id                   UUID PRIMARY KEY,
    purchase_invoice_id  UUID NOT NULL REFERENCES purchase_invoices(id),
    sort_order           INTEGER NOT NULL DEFAULT 0,
    goods_receipt_id     UUID NOT NULL REFERENCES goods_receipts(id),
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

COMMENT ON TABLE purchase_invoice_receipts IS '進項憑證關聯的進貨單（§8.11，明細子表 purchase_invoice_receipt）';
COMMENT ON COLUMN purchase_invoice_receipts.purchase_invoice_id IS '所屬憑證（不是欄位）';
COMMENT ON COLUMN purchase_invoice_receipts.goods_receipt_id IS '已過帳、同廠商的進貨單；同一類憑證只能一張有效（hammer_core 檢查）';

CREATE INDEX idx_purchase_invoice_receipts_purchase_invoice_id
    ON purchase_invoice_receipts (purchase_invoice_id, sort_order, id) WHERE deleted_at IS NULL;
CREATE INDEX idx_purchase_invoice_receipts_goods_receipt_id ON purchase_invoice_receipts (goods_receipt_id);
CREATE INDEX idx_purchase_invoice_receipts_custom_data ON purchase_invoice_receipts USING GIN (custom_data);

CREATE TRIGGER trg_purchase_invoice_receipts_set_updated_at
    BEFORE UPDATE ON purchase_invoice_receipts
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- purchase_allowances（進貨折讓，系統單據 purchase_allowance）
-- ============================================================================

CREATE TABLE purchase_allowances (
    id                   UUID PRIMARY KEY,
    record_no            TEXT NOT NULL,
    purchase_invoice_id  UUID NOT NULL REFERENCES purchase_invoices(id),
    supplier_id          UUID NOT NULL REFERENCES suppliers(id),
    allowance_date       DATE NOT NULL,
    tax_period           TEXT NOT NULL,
    tax_type             TEXT NOT NULL REFERENCES tax_rates(code),
    purchase_return_id   UUID REFERENCES purchase_returns(id),
    subtotal             NUMERIC(18,4) NOT NULL CHECK (subtotal >= 0),
    tax_amount           NUMERIC(18,4) NOT NULL CHECK (tax_amount >= 0),
    total                NUMERIC(18,4) NOT NULL DEFAULT 0,
    status               TEXT NOT NULL DEFAULT 'draft'
                         CHECK (status IN ('draft', 'posted', 'void')),
    reason               TEXT,
    void_date            DATE,
    void_reason          TEXT,
    notes                TEXT,
    posted_at            TIMESTAMPTZ,
    posted_by            UUID REFERENCES users(id),
    voided_at            TIMESTAMPTZ,
    voided_by            UUID REFERENCES users(id),
    custom_data          JSONB NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(custom_data) = 'object'),
    formula_errors       JSONB CHECK (formula_errors IS NULL OR jsonb_typeof(formula_errors) = 'object'),
    -- 共通欄位（§8.1）
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by           UUID REFERENCES users(id),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by           UUID REFERENCES users(id),
    deleted_at           TIMESTAMPTZ,
    version              INTEGER NOT NULL DEFAULT 1,

    CONSTRAINT purchase_allowances_posted_status_check
        CHECK ((status = 'draft') = (posted_at IS NULL)),
    CONSTRAINT purchase_allowances_voided_status_check
        CHECK ((status = 'void') = (voided_at IS NOT NULL)),
    CONSTRAINT purchase_allowances_posted_by_check
        CHECK ((posted_at IS NULL) = (posted_by IS NULL)),
    CONSTRAINT purchase_allowances_voided_by_check
        CHECK ((voided_at IS NULL) = (voided_by IS NULL)),
    CONSTRAINT purchase_allowances_void_date_check
        CHECK ((status = 'void') = (void_date IS NOT NULL)),
    CONSTRAINT purchase_allowances_void_reason_check
        CHECK ((void_date IS NULL) = (void_reason IS NULL))
);

COMMENT ON TABLE purchase_allowances IS '進貨折讓（§8.11，系統單據 purchase_allowance）：沒有品名明細，只記表頭金額';
COMMENT ON COLUMN purchase_allowances.record_no IS '系統單號（不是欄位）';
COMMENT ON COLUMN purchase_allowances.purchase_invoice_id IS '原憑證（必須已登錄、不可是海關繳納證）；建立後不可更換';
COMMENT ON COLUMN purchase_allowances.supplier_id IS '= 憑證的廠商，系統維護';
COMMENT ON COLUMN purchase_allowances.tax_type IS '= 憑證的稅別，系統維護';
COMMENT ON COLUMN purchase_allowances.purchase_return_id IS '關聯的進貨退回（可不填）；一張退回只在一張有效折讓中（hammer_core 檢查）';
COMMENT ON COLUMN purchase_allowances.total IS 'subtotal + tax_amount，系統維護';

CREATE UNIQUE INDEX uq_purchase_allowances_record_no ON purchase_allowances (record_no);
CREATE INDEX idx_purchase_allowances_allowance_date ON purchase_allowances (allowance_date, id);
CREATE INDEX idx_purchase_allowances_status ON purchase_allowances (status);
CREATE INDEX idx_purchase_allowances_supplier_id ON purchase_allowances (supplier_id);
CREATE INDEX idx_purchase_allowances_purchase_invoice_id ON purchase_allowances (purchase_invoice_id);
CREATE INDEX idx_purchase_allowances_purchase_return_id ON purchase_allowances (purchase_return_id);
CREATE INDEX idx_purchase_allowances_custom_data ON purchase_allowances USING GIN (custom_data);

CREATE TRIGGER trg_purchase_allowances_set_updated_at
    BEFORE UPDATE ON purchase_allowances
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- 系統資料類別（§8.11「資料類別」）
-- ============================================================================

INSERT INTO entity_definitions
    (id, key, label, kind, is_system, storage, table_name, parent_entity_id,
     numbering_pattern, title_template, sort_order)
VALUES
    ('01a2a0c0-4c50-7000-96aa-6889f2e48c06', 'sales_invoice', '銷項發票', 'document', true, 'table',
     'sales_invoices', NULL, 'SI{yyyyMM}-{0000}', '{invoice_no} {buyer_name}', 300),
    ('01a2a0c0-4c50-7001-8aa7-3429c8ee98e9', 'sales_invoice_line', '發票明細', 'detail', true, 'table',
     'sales_invoice_lines', '01a2a0c0-4c50-7000-96aa-6889f2e48c06', NULL, '{description}', 0),
    ('01a2a0c0-4c50-7002-b199-8e4a971aa379', 'sales_invoice_shipment', '發票出貨單', 'detail', true, 'table',
     'sales_invoice_shipments', '01a2a0c0-4c50-7000-96aa-6889f2e48c06', NULL, '{shipment_id.record_no}', 0),
    ('01a2a0c0-4c50-7003-a2d1-475d2a81f2f0', 'sales_allowance', '銷貨折讓', 'document', true, 'table',
     'sales_allowances', NULL, 'SA{yyyyMM}-{0000}', '{record_no} {customer_id.name}', 310),
    ('01a2a0c0-4c50-7004-97b7-2aab6b34c734', 'sales_allowance_line', '折讓明細', 'detail', true, 'table',
     'sales_allowance_lines', '01a2a0c0-4c50-7003-a2d1-475d2a81f2f0', NULL, '{description}', 0),
    ('01a2a0c0-4c50-7005-a9e7-e84dc75f60c7', 'purchase_invoice', '進項憑證', 'document', true, 'table',
     'purchase_invoices', NULL, 'PI{yyyyMM}-{0000}', '{invoice_no} {supplier_id.name}', 320),
    ('01a2a0c0-4c50-7006-bd7f-df6239c73510', 'purchase_invoice_receipt', '憑證進貨單', 'detail', true, 'table',
     'purchase_invoice_receipts', '01a2a0c0-4c50-7005-a9e7-e84dc75f60c7', NULL, '{goods_receipt_id.record_no}', 0),
    ('01a2a0c0-4c50-7007-b809-f3c2d74cc57f', 'purchase_allowance', '進貨折讓', 'document', true, 'table',
     'purchase_allowances', NULL, 'PA{yyyyMM}-{0000}', '{record_no} {supplier_id.name}', 330);


-- ============================================================================
-- 系統欄位（sort_order 依表中順序 10、20、30…；唯讀的為系統維護的欄位，§7.1）
-- ============================================================================

-- sales_invoice
INSERT INTO field_definitions
    (id, entity_id, field_key, label, data_type, is_system, storage, column_name,
     is_required, is_searchable, is_visible, is_readonly, default_value, validation, options,
     decimal_places, relation_entity_id, ui_widget, sort_order)
VALUES
    ('01a2a0c0-4c50-7008-b89a-40ea8db0763b', '01a2a0c0-4c50-7000-96aa-6889f2e48c06', 'invoice_type', '發票種類', 'select', true, 'column', 'invoice_type',
     true, false, true, false, '"triplicate"'::jsonb, NULL, '[{"value":"triplicate","label":"三聯式"},{"value":"duplicate","label":"二聯式"}]'::jsonb,
     NULL, NULL, NULL, 10),
    ('01a2a0c0-4c50-7009-b9ca-ea9d2ec1544a', '01a2a0c0-4c50-7000-96aa-6889f2e48c06', 'invoice_no', '發票號碼', 'text', true, 'column', 'invoice_no',
     true, true, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 20),
    ('01a2a0c0-4c50-700a-a9b7-ebb4ef653cd3', '01a2a0c0-4c50-7000-96aa-6889f2e48c06', 'invoice_date', '發票日期', 'date', true, 'column', 'invoice_date',
     true, false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 30),
    ('01a2a0c0-4c50-700b-a362-a9627829928b', '01a2a0c0-4c50-7000-96aa-6889f2e48c06', 'tax_period', '期別', 'text', true, 'column', 'tax_period',
     false, false, true, true, NULL, NULL, NULL,
     NULL, NULL, NULL, 40),
    ('01a2a0c0-4c50-700c-bcd5-1c2f88f34906', '01a2a0c0-4c50-7000-96aa-6889f2e48c06', 'customer_id', '客戶', 'relation', true, 'column', 'customer_id',
     true, false, true, false, NULL, NULL, NULL,
     NULL, '01a1e516-3404-7153-bc28-b536c04db4db', NULL, 50),
    ('01a2a0c0-4c50-700d-b9c2-79303f130e41', '01a2a0c0-4c50-7000-96aa-6889f2e48c06', 'buyer_tax_id', '買受人統一編號', 'text', true, 'column', 'buyer_tax_id',
     false, true, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 60),
    ('01a2a0c0-4c50-700e-b387-ac6c447165a5', '01a2a0c0-4c50-7000-96aa-6889f2e48c06', 'buyer_name', '買受人', 'text', true, 'column', 'buyer_name',
     false, false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 70),
    ('01a2a0c0-4c50-700f-9bee-34b33f8ef080', '01a2a0c0-4c50-7000-96aa-6889f2e48c06', 'buyer_address', '買受人地址', 'text', true, 'column', 'buyer_address',
     false, false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 80),
    ('01a2a0c0-4c50-7010-9c69-5a83fa83dd13', '01a2a0c0-4c50-7000-96aa-6889f2e48c06', 'tax_type', '稅別', 'select', true, 'column', 'tax_type',
     true, false, true, false, NULL, NULL, '[{"value":"taxable","label":"應稅"},{"value":"zero_rated","label":"零稅率"},{"value":"exempt","label":"免稅"}]'::jsonb,
     NULL, NULL, NULL, 90),
    ('01a2a0c0-4c50-7011-88f6-f5d390a02c58', '01a2a0c0-4c50-7000-96aa-6889f2e48c06', 'tax_amount_manual', '稅額（手動）', 'money', true, 'column', 'tax_amount_manual',
     false, false, true, false, NULL, '{"min":"0"}'::jsonb, NULL,
     4, NULL, NULL, 100),
    ('01a2a0c0-4c50-7012-aae7-40d18fb25e86', '01a2a0c0-4c50-7000-96aa-6889f2e48c06', 'status', '狀態', 'select', true, 'column', 'status',
     false, false, true, true, NULL, NULL, '[{"value":"draft","label":"草稿"},{"value":"posted","label":"已開立"},{"value":"void","label":"已作廢"}]'::jsonb,
     NULL, NULL, NULL, 110),
    ('01a2a0c0-4c50-7013-8843-34515aa7cf0b', '01a2a0c0-4c50-7000-96aa-6889f2e48c06', 'subtotal', '未稅金額', 'money', true, 'column', 'subtotal',
     false, false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 120),
    ('01a2a0c0-4c50-7014-9e11-cdacf5a59796', '01a2a0c0-4c50-7000-96aa-6889f2e48c06', 'tax_amount', '稅額', 'money', true, 'column', 'tax_amount',
     false, false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 130),
    ('01a2a0c0-4c50-7015-b5d9-be183bfce7ae', '01a2a0c0-4c50-7000-96aa-6889f2e48c06', 'total', '總計', 'money', true, 'column', 'total',
     false, false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 140),
    ('01a2a0c0-4c50-7016-ae50-5bb96605a5a6', '01a2a0c0-4c50-7000-96aa-6889f2e48c06', 'void_date', '作廢日期', 'date', true, 'column', 'void_date',
     false, false, true, true, NULL, NULL, NULL,
     NULL, NULL, NULL, 150),
    ('01a2a0c0-4c50-7017-871a-c3a2536c5cfd', '01a2a0c0-4c50-7000-96aa-6889f2e48c06', 'void_reason', '作廢原因', 'text', true, 'column', 'void_reason',
     false, false, true, true, NULL, NULL, NULL,
     NULL, NULL, NULL, 160),
    ('01a2a0c0-4c50-7018-9e69-b2d5dfd17889', '01a2a0c0-4c50-7000-96aa-6889f2e48c06', 'notes', '備註', 'long_text', true, 'column', 'notes',
     false, false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 170);

-- sales_invoice_line
INSERT INTO field_definitions
    (id, entity_id, field_key, label, data_type, is_system, storage, column_name,
     is_required, is_searchable, is_visible, is_readonly, default_value, validation, options,
     decimal_places, relation_entity_id, ui_widget, sort_order)
VALUES
    ('01a2a0c0-4c50-7019-ae86-302d742a88ec', '01a2a0c0-4c50-7001-8aa7-3429c8ee98e9', 'product_id', '商品', 'relation', true, 'column', 'product_id',
     false, false, true, false, NULL, NULL, NULL,
     NULL, '01a1e516-3401-7432-acb3-9cc3ac6a7db9', NULL, 10),
    ('01a2a0c0-4c50-701a-b76a-e2c74fac308d', '01a2a0c0-4c50-7001-8aa7-3429c8ee98e9', 'description', '品名', 'text', true, 'column', 'description',
     true, false, true, false, NULL, '{"max_length":200}'::jsonb, NULL,
     NULL, NULL, NULL, 20),
    ('01a2a0c0-4c50-701b-99a7-dd9324f8ce78', '01a2a0c0-4c50-7001-8aa7-3429c8ee98e9', 'qty', '數量', 'decimal', true, 'column', 'qty',
     true, false, true, false, NULL, '{"min":"0"}'::jsonb, NULL,
     4, NULL, NULL, 30),
    ('01a2a0c0-4c50-701c-8403-0aaf44ff35cc', '01a2a0c0-4c50-7001-8aa7-3429c8ee98e9', 'unit_price', '單價', 'money', true, 'column', 'unit_price',
     true, false, true, false, NULL, '{"min":"0"}'::jsonb, NULL,
     4, NULL, NULL, 40),
    ('01a2a0c0-4c50-701d-8db4-13ba079a7afb', '01a2a0c0-4c50-7001-8aa7-3429c8ee98e9', 'amount', '金額', 'money', true, 'column', 'amount',
     false, false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 50),
    ('01a2a0c0-4c50-701e-a2ee-e43fb0323186', '01a2a0c0-4c50-7001-8aa7-3429c8ee98e9', 'notes', '備註', 'long_text', true, 'column', 'notes',
     false, false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 60);

-- sales_invoice_shipment
INSERT INTO field_definitions
    (id, entity_id, field_key, label, data_type, is_system, storage, column_name,
     is_required, is_searchable, is_visible, is_readonly, default_value, validation, options,
     decimal_places, relation_entity_id, ui_widget, sort_order)
VALUES
    ('01a2a0c0-4c50-701f-b14d-5131cac9b56a', '01a2a0c0-4c50-7002-b199-8e4a971aa379', 'shipment_id', '出貨單', 'relation', true, 'column', 'shipment_id',
     true, false, true, false, NULL, NULL, NULL,
     NULL, '01a1f9d2-7b30-7002-bbf4-a37505aebf4d', NULL, 10);

-- sales_allowance
INSERT INTO field_definitions
    (id, entity_id, field_key, label, data_type, is_system, storage, column_name,
     is_required, is_searchable, is_visible, is_readonly, default_value, validation, options,
     decimal_places, relation_entity_id, ui_widget, sort_order)
VALUES
    ('01a2a0c0-4c50-7020-8444-50b8311669ab', '01a2a0c0-4c50-7003-a2d1-475d2a81f2f0', 'sales_invoice_id', '原發票', 'relation', true, 'column', 'sales_invoice_id',
     true, false, true, false, NULL, NULL, NULL,
     NULL, '01a2a0c0-4c50-7000-96aa-6889f2e48c06', NULL, 10),
    ('01a2a0c0-4c50-7021-99e4-abe4fc9b5a5a', '01a2a0c0-4c50-7003-a2d1-475d2a81f2f0', 'customer_id', '客戶', 'relation', true, 'column', 'customer_id',
     false, false, true, true, NULL, NULL, NULL,
     NULL, '01a1e516-3404-7153-bc28-b536c04db4db', NULL, 20),
    ('01a2a0c0-4c50-7022-8507-5f40f7b39f4d', '01a2a0c0-4c50-7003-a2d1-475d2a81f2f0', 'allowance_date', '折讓日期', 'date', true, 'column', 'allowance_date',
     true, false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 30),
    ('01a2a0c0-4c50-7023-b997-e946817efa3b', '01a2a0c0-4c50-7003-a2d1-475d2a81f2f0', 'tax_period', '期別', 'text', true, 'column', 'tax_period',
     false, false, true, true, NULL, NULL, NULL,
     NULL, NULL, NULL, 40),
    ('01a2a0c0-4c50-7024-941f-8bea52eb8f2a', '01a2a0c0-4c50-7003-a2d1-475d2a81f2f0', 'tax_type', '稅別', 'select', true, 'column', 'tax_type',
     false, false, true, true, NULL, NULL, '[{"value":"taxable","label":"應稅"},{"value":"zero_rated","label":"零稅率"},{"value":"exempt","label":"免稅"}]'::jsonb,
     NULL, NULL, NULL, 50),
    ('01a2a0c0-4c50-7025-ad8c-ea4afa5b45f2', '01a2a0c0-4c50-7003-a2d1-475d2a81f2f0', 'sales_return_id', '銷貨退回', 'relation', true, 'column', 'sales_return_id',
     false, false, true, false, NULL, NULL, NULL,
     NULL, '01a1f9d2-7b30-7004-a2fd-f5fe1974d3b8', NULL, 60),
    ('01a2a0c0-4c50-7026-95b0-cc53f9539871', '01a2a0c0-4c50-7003-a2d1-475d2a81f2f0', 'tax_amount_manual', '稅額（手動）', 'money', true, 'column', 'tax_amount_manual',
     false, false, true, false, NULL, '{"min":"0"}'::jsonb, NULL,
     4, NULL, NULL, 70),
    ('01a2a0c0-4c50-7027-96f6-d63b35be02a5', '01a2a0c0-4c50-7003-a2d1-475d2a81f2f0', 'status', '狀態', 'select', true, 'column', 'status',
     false, false, true, true, NULL, NULL, '[{"value":"draft","label":"草稿"},{"value":"posted","label":"已開立"},{"value":"void","label":"已作廢"}]'::jsonb,
     NULL, NULL, NULL, 80),
    ('01a2a0c0-4c50-7028-a120-ef48d0d56aa9', '01a2a0c0-4c50-7003-a2d1-475d2a81f2f0', 'subtotal', '折讓未稅金額', 'money', true, 'column', 'subtotal',
     false, false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 90),
    ('01a2a0c0-4c50-7029-a4c3-c2527dceead4', '01a2a0c0-4c50-7003-a2d1-475d2a81f2f0', 'tax_amount', '折讓稅額', 'money', true, 'column', 'tax_amount',
     false, false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 100),
    ('01a2a0c0-4c50-702a-b8e5-215fd705cd38', '01a2a0c0-4c50-7003-a2d1-475d2a81f2f0', 'total', '折讓總計', 'money', true, 'column', 'total',
     false, false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 110),
    ('01a2a0c0-4c50-702b-8f99-7227bdfa7204', '01a2a0c0-4c50-7003-a2d1-475d2a81f2f0', 'reason', '折讓原因', 'text', true, 'column', 'reason',
     false, false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 120),
    ('01a2a0c0-4c50-702c-92d8-554028d4d706', '01a2a0c0-4c50-7003-a2d1-475d2a81f2f0', 'void_date', '作廢日期', 'date', true, 'column', 'void_date',
     false, false, true, true, NULL, NULL, NULL,
     NULL, NULL, NULL, 130),
    ('01a2a0c0-4c50-702d-9d97-14f1e8310958', '01a2a0c0-4c50-7003-a2d1-475d2a81f2f0', 'void_reason', '作廢原因', 'text', true, 'column', 'void_reason',
     false, false, true, true, NULL, NULL, NULL,
     NULL, NULL, NULL, 140),
    ('01a2a0c0-4c50-702e-bc89-37e84c6a1cc4', '01a2a0c0-4c50-7003-a2d1-475d2a81f2f0', 'notes', '備註', 'long_text', true, 'column', 'notes',
     false, false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 150);

-- sales_allowance_line
INSERT INTO field_definitions
    (id, entity_id, field_key, label, data_type, is_system, storage, column_name,
     is_required, is_searchable, is_visible, is_readonly, default_value, validation, options,
     decimal_places, relation_entity_id, ui_widget, sort_order)
VALUES
    ('01a2a0c0-4c50-702f-9959-63b0ce5be3ce', '01a2a0c0-4c50-7004-97b7-2aab6b34c734', 'product_id', '商品', 'relation', true, 'column', 'product_id',
     false, false, true, false, NULL, NULL, NULL,
     NULL, '01a1e516-3401-7432-acb3-9cc3ac6a7db9', NULL, 10),
    ('01a2a0c0-4c50-7030-a5ff-bbb9f5ed97b2', '01a2a0c0-4c50-7004-97b7-2aab6b34c734', 'description', '品名', 'text', true, 'column', 'description',
     true, false, true, false, NULL, '{"max_length":200}'::jsonb, NULL,
     NULL, NULL, NULL, 20),
    ('01a2a0c0-4c50-7031-b9d6-f6bd9e40b51f', '01a2a0c0-4c50-7004-97b7-2aab6b34c734', 'qty', '數量', 'decimal', true, 'column', 'qty',
     true, false, true, false, NULL, '{"min":"0"}'::jsonb, NULL,
     4, NULL, NULL, 30),
    ('01a2a0c0-4c50-7032-ad59-cb597b3c0a76', '01a2a0c0-4c50-7004-97b7-2aab6b34c734', 'unit_price', '單價', 'money', true, 'column', 'unit_price',
     true, false, true, false, NULL, '{"min":"0"}'::jsonb, NULL,
     4, NULL, NULL, 40),
    ('01a2a0c0-4c50-7033-ae2e-1fd69b4d82f2', '01a2a0c0-4c50-7004-97b7-2aab6b34c734', 'amount', '金額', 'money', true, 'column', 'amount',
     false, false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 50),
    ('01a2a0c0-4c50-7034-9328-82e80e5888c2', '01a2a0c0-4c50-7004-97b7-2aab6b34c734', 'notes', '備註', 'long_text', true, 'column', 'notes',
     false, false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 60);

-- purchase_invoice
INSERT INTO field_definitions
    (id, entity_id, field_key, label, data_type, is_system, storage, column_name,
     is_required, is_searchable, is_visible, is_readonly, default_value, validation, options,
     decimal_places, relation_entity_id, ui_widget, sort_order)
VALUES
    ('01a2a0c0-4c50-7035-813c-f9fdbfca23cc', '01a2a0c0-4c50-7005-a9e7-e84dc75f60c7', 'doc_type', '憑證種類', 'select', true, 'column', 'doc_type',
     true, false, true, false, '"triplicate"'::jsonb, NULL, '[{"value":"triplicate","label":"三聯式發票"},{"value":"electronic","label":"電子發票"},{"value":"customs","label":"海關代徵營業稅繳納證"}]'::jsonb,
     NULL, NULL, NULL, 10),
    ('01a2a0c0-4c50-7036-a6b3-75c58f817915', '01a2a0c0-4c50-7005-a9e7-e84dc75f60c7', 'invoice_no', '憑證號碼', 'text', true, 'column', 'invoice_no',
     true, true, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 20),
    ('01a2a0c0-4c50-7037-8ceb-8ffd9c61fc24', '01a2a0c0-4c50-7005-a9e7-e84dc75f60c7', 'invoice_date', '憑證日期', 'date', true, 'column', 'invoice_date',
     true, false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 30),
    ('01a2a0c0-4c50-7038-9cdd-2c4af9c1f0c5', '01a2a0c0-4c50-7005-a9e7-e84dc75f60c7', 'tax_period', '期別', 'text', true, 'column', 'tax_period',
     false, false, true, true, NULL, NULL, NULL,
     NULL, NULL, NULL, 40),
    ('01a2a0c0-4c50-7039-8489-cb4cfee805f1', '01a2a0c0-4c50-7005-a9e7-e84dc75f60c7', 'supplier_id', '廠商', 'relation', true, 'column', 'supplier_id',
     true, false, true, false, NULL, NULL, NULL,
     NULL, '01a1e516-3406-71fc-8cf2-9ce47caf0e68', NULL, 50),
    ('01a2a0c0-4c50-703a-8575-f8b394aea00d', '01a2a0c0-4c50-7005-a9e7-e84dc75f60c7', 'seller_tax_id', '賣方統一編號', 'text', true, 'column', 'seller_tax_id',
     false, false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 60),
    ('01a2a0c0-4c50-703b-b769-2f4edbe1d8d3', '01a2a0c0-4c50-7005-a9e7-e84dc75f60c7', 'tax_type', '稅別', 'select', true, 'column', 'tax_type',
     true, false, true, false, '"taxable"'::jsonb, NULL, '[{"value":"taxable","label":"應稅"},{"value":"zero_rated","label":"零稅率"},{"value":"exempt","label":"免稅"}]'::jsonb,
     NULL, NULL, NULL, 70),
    ('01a2a0c0-4c50-703c-ae03-aeb6ec8e32cb', '01a2a0c0-4c50-7005-a9e7-e84dc75f60c7', 'subtotal', '未稅金額', 'money', true, 'column', 'subtotal',
     true, false, true, false, NULL, '{"min":"0"}'::jsonb, NULL,
     4, NULL, NULL, 80),
    ('01a2a0c0-4c50-703d-966e-94615da9ceba', '01a2a0c0-4c50-7005-a9e7-e84dc75f60c7', 'tax_amount', '稅額', 'money', true, 'column', 'tax_amount',
     true, false, true, false, NULL, '{"min":"0"}'::jsonb, NULL,
     4, NULL, NULL, 90),
    ('01a2a0c0-4c50-703e-8042-ae98b031a520', '01a2a0c0-4c50-7005-a9e7-e84dc75f60c7', 'total', '總計', 'money', true, 'column', 'total',
     false, false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 100),
    ('01a2a0c0-4c50-703f-bb81-69322a745945', '01a2a0c0-4c50-7005-a9e7-e84dc75f60c7', 'deductible', '可扣抵', 'boolean', true, 'column', 'deductible',
     true, false, true, false, 'true'::jsonb, NULL, NULL,
     NULL, NULL, NULL, 110),
    ('01a2a0c0-4c50-7040-861e-7e72b03a8911', '01a2a0c0-4c50-7005-a9e7-e84dc75f60c7', 'status', '狀態', 'select', true, 'column', 'status',
     false, false, true, true, NULL, NULL, '[{"value":"draft","label":"草稿"},{"value":"posted","label":"已登錄"},{"value":"void","label":"已作廢"}]'::jsonb,
     NULL, NULL, NULL, 120),
    ('01a2a0c0-4c50-7041-b2de-ceb6c5cacb4a', '01a2a0c0-4c50-7005-a9e7-e84dc75f60c7', 'void_date', '作廢日期', 'date', true, 'column', 'void_date',
     false, false, true, true, NULL, NULL, NULL,
     NULL, NULL, NULL, 130),
    ('01a2a0c0-4c50-7042-abb4-31baeebe9122', '01a2a0c0-4c50-7005-a9e7-e84dc75f60c7', 'void_reason', '作廢原因', 'text', true, 'column', 'void_reason',
     false, false, true, true, NULL, NULL, NULL,
     NULL, NULL, NULL, 140),
    ('01a2a0c0-4c50-7043-92cf-2be144048d6e', '01a2a0c0-4c50-7005-a9e7-e84dc75f60c7', 'notes', '備註', 'long_text', true, 'column', 'notes',
     false, false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 150);

-- purchase_invoice_receipt
INSERT INTO field_definitions
    (id, entity_id, field_key, label, data_type, is_system, storage, column_name,
     is_required, is_searchable, is_visible, is_readonly, default_value, validation, options,
     decimal_places, relation_entity_id, ui_widget, sort_order)
VALUES
    ('01a2a0c0-4c50-7044-8c7f-b3e040131c66', '01a2a0c0-4c50-7006-bd7f-df6239c73510', 'goods_receipt_id', '進貨單', 'relation', true, 'column', 'goods_receipt_id',
     true, false, true, false, NULL, NULL, NULL,
     NULL, '01a1f4c0-5e10-7002-a3f6-25702f618e4d', NULL, 10);

-- purchase_allowance
INSERT INTO field_definitions
    (id, entity_id, field_key, label, data_type, is_system, storage, column_name,
     is_required, is_searchable, is_visible, is_readonly, default_value, validation, options,
     decimal_places, relation_entity_id, ui_widget, sort_order)
VALUES
    ('01a2a0c0-4c50-7045-8519-71f7f6edb37d', '01a2a0c0-4c50-7007-b809-f3c2d74cc57f', 'purchase_invoice_id', '原憑證', 'relation', true, 'column', 'purchase_invoice_id',
     true, false, true, false, NULL, NULL, NULL,
     NULL, '01a2a0c0-4c50-7005-a9e7-e84dc75f60c7', NULL, 10),
    ('01a2a0c0-4c50-7046-b99a-aaa49598d1b4', '01a2a0c0-4c50-7007-b809-f3c2d74cc57f', 'supplier_id', '廠商', 'relation', true, 'column', 'supplier_id',
     false, false, true, true, NULL, NULL, NULL,
     NULL, '01a1e516-3406-71fc-8cf2-9ce47caf0e68', NULL, 20),
    ('01a2a0c0-4c50-7047-b7c9-45d9ab4c2cd9', '01a2a0c0-4c50-7007-b809-f3c2d74cc57f', 'allowance_date', '折讓日期', 'date', true, 'column', 'allowance_date',
     true, false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 30),
    ('01a2a0c0-4c50-7048-abaa-6985e6355613', '01a2a0c0-4c50-7007-b809-f3c2d74cc57f', 'tax_period', '期別', 'text', true, 'column', 'tax_period',
     false, false, true, true, NULL, NULL, NULL,
     NULL, NULL, NULL, 40),
    ('01a2a0c0-4c50-7049-b1e0-48ceb9a82ce3', '01a2a0c0-4c50-7007-b809-f3c2d74cc57f', 'tax_type', '稅別', 'select', true, 'column', 'tax_type',
     false, false, true, true, NULL, NULL, '[{"value":"taxable","label":"應稅"},{"value":"zero_rated","label":"零稅率"},{"value":"exempt","label":"免稅"}]'::jsonb,
     NULL, NULL, NULL, 50),
    ('01a2a0c0-4c50-704a-a2a8-c5813d2e6733', '01a2a0c0-4c50-7007-b809-f3c2d74cc57f', 'purchase_return_id', '進貨退回', 'relation', true, 'column', 'purchase_return_id',
     false, false, true, false, NULL, NULL, NULL,
     NULL, '01a1f4c0-5e10-7004-9f63-b2ea144e4b01', NULL, 60),
    ('01a2a0c0-4c50-704b-b706-3e93b3f0370d', '01a2a0c0-4c50-7007-b809-f3c2d74cc57f', 'subtotal', '折讓未稅金額', 'money', true, 'column', 'subtotal',
     true, false, true, false, NULL, '{"min":"0"}'::jsonb, NULL,
     4, NULL, NULL, 70),
    ('01a2a0c0-4c50-704c-83e3-70be0f2fee7d', '01a2a0c0-4c50-7007-b809-f3c2d74cc57f', 'tax_amount', '折讓稅額', 'money', true, 'column', 'tax_amount',
     true, false, true, false, NULL, '{"min":"0"}'::jsonb, NULL,
     4, NULL, NULL, 80),
    ('01a2a0c0-4c50-704d-a8f9-6d217508b7f9', '01a2a0c0-4c50-7007-b809-f3c2d74cc57f', 'total', '折讓總計', 'money', true, 'column', 'total',
     false, false, true, true, NULL, NULL, NULL,
     4, NULL, NULL, 90),
    ('01a2a0c0-4c50-704e-b058-7a35cd815e1d', '01a2a0c0-4c50-7007-b809-f3c2d74cc57f', 'status', '狀態', 'select', true, 'column', 'status',
     false, false, true, true, NULL, NULL, '[{"value":"draft","label":"草稿"},{"value":"posted","label":"已開立"},{"value":"void","label":"已作廢"}]'::jsonb,
     NULL, NULL, NULL, 100),
    ('01a2a0c0-4c50-704f-bf33-683a8a7f2895', '01a2a0c0-4c50-7007-b809-f3c2d74cc57f', 'reason', '折讓原因', 'text', true, 'column', 'reason',
     false, false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 110),
    ('01a2a0c0-4c50-7050-9ce6-7c63ee91dcaa', '01a2a0c0-4c50-7007-b809-f3c2d74cc57f', 'void_date', '作廢日期', 'date', true, 'column', 'void_date',
     false, false, true, true, NULL, NULL, NULL,
     NULL, NULL, NULL, 120),
    ('01a2a0c0-4c50-7051-a323-6b2eb7a86ae8', '01a2a0c0-4c50-7007-b809-f3c2d74cc57f', 'void_reason', '作廢原因', 'text', true, 'column', 'void_reason',
     false, false, true, true, NULL, NULL, NULL,
     NULL, NULL, NULL, 130),
    ('01a2a0c0-4c50-7052-ae72-57be4b542547', '01a2a0c0-4c50-7007-b809-f3c2d74cc57f', 'notes', '備註', 'long_text', true, 'column', 'notes',
     false, false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 140);

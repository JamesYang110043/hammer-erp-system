-- 0011_inventory：M4a 庫存資料表與盤點調整單（#128，docs/architecture.md §7.1「系統單據」、§8.10）。
--
-- 建立 stock_adjustments、stock_adjustment_lines（系統單據 stock_adjustment 與其明細子表）、
-- inventory_lots、lot_allocations、inventory_transactions 與檢視表 v_stock_balances，
-- 並把盤點調整單登記到 entity_definitions、field_definitions。
--
-- 盤點調整單依 §8.6「系統資料表的共同規則」：系統欄位 → custom_data → formula_errors → 共通欄位；
-- 單號 record_no 與過帳、作廢的時間與人員不是欄位（§7.1「系統單據」）。
--
-- 庫存三張表不是資料類別（沒有自訂欄位），只能由單據的過帳、作廢寫入。
-- lot_allocations、inventory_transactions 只能新增：以 trigger 拒絕 UPDATE、DELETE、TRUNCATE。
--
-- source_type、ref_doc_type 只列出盤點調整單的代碼；M4b、M4c 的 migration 修改 CHECK 加上各自的代碼。
--
-- 資料類別、欄位定義的 id 為固定的 uuid v7，方便跨環境比對。
-- 欄位定義必須與 architecture.md §8.10 一致，由 crates/db/tests/inventory.rs 檢查。

-- ============================================================================
-- 只新增的資料表：拒絕修改與刪除
-- ============================================================================

CREATE FUNCTION reject_append_only_change() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION '% 只能新增，不可修改或刪除', TG_TABLE_NAME
        USING ERRCODE = 'restrict_violation';
END;
$$;

COMMENT ON FUNCTION reject_append_only_change() IS
    '只新增的資料表（lot_allocations、inventory_transactions）拒絕 UPDATE、DELETE、TRUNCATE（§8.10）';

-- ============================================================================
-- stock_adjustments（盤點與調整單，系統單據 stock_adjustment）
-- ============================================================================

CREATE TABLE stock_adjustments (
    id               UUID PRIMARY KEY,
    record_no        TEXT NOT NULL,
    adjustment_date  DATE NOT NULL,
    adjustment_type  TEXT NOT NULL DEFAULT 'count'
                     CHECK (adjustment_type IN ('opening', 'count', 'adjustment')),
    status           TEXT NOT NULL DEFAULT 'draft'
                     CHECK (status IN ('draft', 'posted', 'void')),
    notes            TEXT,
    posted_at        TIMESTAMPTZ,
    posted_by        UUID REFERENCES users(id),
    voided_at        TIMESTAMPTZ,
    voided_by        UUID REFERENCES users(id),
    custom_data      JSONB NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(custom_data) = 'object'),
    formula_errors   JSONB CHECK (formula_errors IS NULL OR jsonb_typeof(formula_errors) = 'object'),
    -- 共通欄位（§8.1）
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by       UUID REFERENCES users(id),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by       UUID REFERENCES users(id),
    deleted_at       TIMESTAMPTZ,
    version          INTEGER NOT NULL DEFAULT 1,

    CONSTRAINT stock_adjustments_posted_status_check
        CHECK ((status = 'draft') = (posted_at IS NULL)),
    CONSTRAINT stock_adjustments_voided_status_check
        CHECK ((status = 'void') = (voided_at IS NOT NULL)),
    CONSTRAINT stock_adjustments_posted_by_check
        CHECK ((posted_at IS NULL) = (posted_by IS NULL)),
    CONSTRAINT stock_adjustments_voided_by_check
        CHECK ((voided_at IS NULL) = (voided_by IS NULL))
);

COMMENT ON TABLE stock_adjustments IS '盤點與調整單（§8.10，系統單據 stock_adjustment）；也用於期初庫存';
COMMENT ON COLUMN stock_adjustments.record_no IS '單號（不是欄位），依 numbering_pattern 取號；唯一索引含已刪除的資料';
COMMENT ON COLUMN stock_adjustments.adjustment_date IS '調整日期：新批號的 received_date、異動的 txn_date';
COMMENT ON COLUMN stock_adjustments.adjustment_type IS 'opening（期初庫存）/ count（盤點）/ adjustment（一般調整）';
COMMENT ON COLUMN stock_adjustments.status IS 'draft / posted / void，系統維護（§7.1、§8.2）';
COMMENT ON COLUMN stock_adjustments.posted_at IS '過帳時間（不是欄位）';
COMMENT ON COLUMN stock_adjustments.voided_at IS '作廢時間（不是欄位）';

CREATE UNIQUE INDEX uq_stock_adjustments_record_no ON stock_adjustments (record_no);
CREATE INDEX idx_stock_adjustments_adjustment_date ON stock_adjustments (adjustment_date, id);
CREATE INDEX idx_stock_adjustments_status ON stock_adjustments (status);
CREATE INDEX idx_stock_adjustments_custom_data ON stock_adjustments USING GIN (custom_data);

CREATE TRIGGER trg_stock_adjustments_set_updated_at
    BEFORE UPDATE ON stock_adjustments
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- inventory_lots（批號）
-- ============================================================================

CREATE TABLE inventory_lots (
    id              UUID PRIMARY KEY,
    product_id      UUID NOT NULL REFERENCES products(id),
    lot_no          TEXT NOT NULL CHECK (char_length(lot_no) BETWEEN 1 AND 50),
    received_date   DATE NOT NULL,
    source_type     TEXT NOT NULL CHECK (source_type IN ('stock_adjustment_line')),
    source_doc_id   UUID NOT NULL,
    source_line_id  UUID NOT NULL,
    qty_in          NUMERIC(18,4) NOT NULL CHECK (qty_in > 0),
    qty_remaining   NUMERIC(18,4) NOT NULL CHECK (qty_remaining >= 0),
    unit_cost_twd   NUMERIC(18,4) NOT NULL CHECK (unit_cost_twd >= 0),
    -- 共通欄位（§8.1）
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by      UUID REFERENCES users(id),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by      UUID REFERENCES users(id),
    deleted_at      TIMESTAMPTZ,
    version         INTEGER NOT NULL DEFAULT 1,

    CONSTRAINT inventory_lots_source_line_key UNIQUE (source_type, source_line_id),
    CONSTRAINT inventory_lots_deleted_empty_check
        CHECK (deleted_at IS NULL OR qty_remaining = 0)
);

COMMENT ON TABLE inventory_lots IS '批號（§8.10）；只有 goods 商品有批號';
COMMENT ON COLUMN inventory_lots.lot_no IS '批號；去除前後空白、最長 50 字元；同商品未刪除的批號不分大小寫不可重複';
COMMENT ON COLUMN inventory_lots.received_date IS '入庫日期 = 來源單據的單據日期；先進先出的排序依據';
COMMENT ON COLUMN inventory_lots.source_type IS '產生此批號的單據明細子表代碼';
COMMENT ON COLUMN inventory_lots.qty_in IS '原始入庫數量，建立後不變';
COMMENT ON COLUMN inventory_lots.qty_remaining IS '目前數量；可能大於 qty_in（盤盈加回、銷貨退回）';
COMMENT ON COLUMN inventory_lots.unit_cost_twd IS '台幣單位成本，建立後不變';
COMMENT ON COLUMN inventory_lots.version IS '每次異動 + 1，不用於檢查（一律 FOR UPDATE 鎖定後修改）';

CREATE UNIQUE INDEX uq_inventory_lots_product_lot_no
    ON inventory_lots (product_id, lower(lot_no)) WHERE deleted_at IS NULL;
CREATE INDEX idx_inventory_lots_fifo
    ON inventory_lots (product_id, received_date, id)
    WHERE qty_remaining > 0 AND deleted_at IS NULL;
CREATE INDEX idx_inventory_lots_source_doc_id ON inventory_lots (source_doc_id);

CREATE TRIGGER trg_inventory_lots_set_updated_at
    BEFORE UPDATE ON inventory_lots
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- stock_adjustment_lines（明細子表 stock_adjustment_line）
-- ============================================================================

CREATE TABLE stock_adjustment_lines (
    id                   UUID PRIMARY KEY,
    stock_adjustment_id  UUID NOT NULL REFERENCES stock_adjustments(id),
    sort_order           INTEGER NOT NULL DEFAULT 0,
    product_id           UUID NOT NULL REFERENCES products(id),
    line_type            TEXT NOT NULL DEFAULT 'gain_new'
                         CHECK (line_type IN ('gain_new', 'gain_existing', 'loss')),
    lot_no               TEXT,
    qty                  NUMERIC(18,4) NOT NULL CHECK (qty > 0),
    unit_cost_twd        NUMERIC(18,4) CHECK (unit_cost_twd >= 0),
    reason               TEXT,
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

COMMENT ON TABLE stock_adjustment_lines IS '盤點調整明細（§8.10，明細子表 stock_adjustment_line）';
COMMENT ON COLUMN stock_adjustment_lines.stock_adjustment_id IS '所屬單據（不是欄位）';
COMMENT ON COLUMN stock_adjustment_lines.sort_order IS '明細順序，從 0 開始（不是欄位）';
COMMENT ON COLUMN stock_adjustment_lines.line_type IS 'gain_new（盤盈：新批號）/ gain_existing（盤盈：加回既有批號）/ loss（盤虧：扣指定批號）';
COMMENT ON COLUMN stock_adjustment_lines.lot_no IS 'gain_new 可留空（過帳時自動產生）；gain_existing、loss 必填，由 hammer_core 檢查';
COMMENT ON COLUMN stock_adjustment_lines.qty IS '一律為正數，增減由 line_type 決定';
COMMENT ON COLUMN stock_adjustment_lines.unit_cost_twd IS 'gain_new 必填；gain_existing、loss 過帳時寫入批號的成本';
COMMENT ON COLUMN stock_adjustment_lines.lot_id IS '過帳時寫入的批號（不是欄位）';

CREATE INDEX idx_stock_adjustment_lines_stock_adjustment_id
    ON stock_adjustment_lines (stock_adjustment_id, sort_order, id) WHERE deleted_at IS NULL;
CREATE INDEX idx_stock_adjustment_lines_product_id ON stock_adjustment_lines (product_id);
CREATE INDEX idx_stock_adjustment_lines_lot_id ON stock_adjustment_lines (lot_id);
CREATE INDEX idx_stock_adjustment_lines_custom_data ON stock_adjustment_lines USING GIN (custom_data);

CREATE TRIGGER trg_stock_adjustment_lines_set_updated_at
    BEFORE UPDATE ON stock_adjustment_lines
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- lot_allocations（批號扣除明細，只新增）
-- ============================================================================

CREATE TABLE lot_allocations (
    id              UUID PRIMARY KEY,
    source_type     TEXT NOT NULL CHECK (source_type IN ('stock_adjustment_line')),
    source_doc_id   UUID NOT NULL,
    source_line_id  UUID NOT NULL,
    lot_id          UUID NOT NULL REFERENCES inventory_lots(id),
    qty             NUMERIC(18,4) NOT NULL CHECK (qty > 0),
    unit_cost_twd   NUMERIC(18,4) NOT NULL CHECK (unit_cost_twd >= 0),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by      UUID REFERENCES users(id)
);

COMMENT ON TABLE lot_allocations IS '批號扣除明細（§8.10）；只新增，作廢時依它加回原批號';
COMMENT ON COLUMN lot_allocations.source_type IS '扣除庫存的單據明細子表代碼';
COMMENT ON COLUMN lot_allocations.unit_cost_twd IS '扣除當下該批號的成本';

CREATE INDEX idx_lot_allocations_source_line ON lot_allocations (source_type, source_line_id);
CREATE INDEX idx_lot_allocations_lot_id ON lot_allocations (lot_id);

CREATE TRIGGER trg_lot_allocations_append_only
    BEFORE UPDATE OR DELETE ON lot_allocations
    FOR EACH ROW EXECUTE FUNCTION reject_append_only_change();
CREATE TRIGGER trg_lot_allocations_no_truncate
    BEFORE TRUNCATE ON lot_allocations
    FOR EACH STATEMENT EXECUTE FUNCTION reject_append_only_change();

-- ============================================================================
-- inventory_transactions（庫存異動流水帳，只新增）
-- ============================================================================

CREATE TABLE inventory_transactions (
    id             UUID PRIMARY KEY,
    txn_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    txn_date       DATE NOT NULL,
    txn_type       TEXT NOT NULL CHECK (txn_type IN (
                       'receipt', 'shipment', 'sales_return', 'purchase_return',
                       'adjustment', 'void_reversal')),
    product_id     UUID NOT NULL REFERENCES products(id),
    lot_id         UUID NOT NULL REFERENCES inventory_lots(id),
    qty_change     NUMERIC(18,4) NOT NULL CHECK (qty_change <> 0),
    unit_cost_twd  NUMERIC(18,4) NOT NULL CHECK (unit_cost_twd >= 0),
    ref_doc_type   TEXT NOT NULL CHECK (ref_doc_type IN ('stock_adjustment')),
    ref_doc_id     UUID NOT NULL,
    ref_line_id    UUID NOT NULL,
    created_by     UUID REFERENCES users(id)
);

COMMENT ON TABLE inventory_transactions IS '庫存異動流水帳（§8.10）；只新增，作廢時新增 void_reversal 的反向異動';
COMMENT ON COLUMN inventory_transactions.txn_at IS '過帳或作廢的時間';
COMMENT ON COLUMN inventory_transactions.txn_date IS '業務日期：過帳時為單據日期；作廢時為作廢當天（Asia/Taipei）';
COMMENT ON COLUMN inventory_transactions.qty_change IS '入庫為正、出庫為負';
COMMENT ON COLUMN inventory_transactions.ref_doc_type IS '來源單據的主表代碼';

CREATE INDEX idx_inventory_transactions_product_date
    ON inventory_transactions (product_id, txn_date, id);
CREATE INDEX idx_inventory_transactions_lot_ref
    ON inventory_transactions (lot_id, ref_doc_type, ref_doc_id);
CREATE INDEX idx_inventory_transactions_ref
    ON inventory_transactions (ref_doc_type, ref_doc_id);

CREATE TRIGGER trg_inventory_transactions_append_only
    BEFORE UPDATE OR DELETE ON inventory_transactions
    FOR EACH ROW EXECUTE FUNCTION reject_append_only_change();
CREATE TRIGGER trg_inventory_transactions_no_truncate
    BEFORE TRUNCATE ON inventory_transactions
    FOR EACH STATEMENT EXECUTE FUNCTION reject_append_only_change();

-- ============================================================================
-- v_stock_balances（檢視表）
-- ============================================================================

CREATE VIEW v_stock_balances AS
SELECT product_id,
       SUM(qty_remaining)                 AS qty_on_hand,
       SUM(qty_remaining * unit_cost_twd) AS stock_value_twd
FROM inventory_lots
WHERE deleted_at IS NULL
GROUP BY product_id;

COMMENT ON VIEW v_stock_balances IS '商品庫存（§8.10）；只包含有批號的商品，stock_value_twd 在 API 回傳時四捨五入到 2 位';

-- ============================================================================
-- 系統資料類別（§8.10「stock_adjustments」）
-- ============================================================================

INSERT INTO entity_definitions
    (id, key, label, kind, is_system, storage, table_name, parent_entity_id,
     numbering_pattern, title_template, sort_order)
VALUES
    ('01a1ea12-f322-7000-8bce-42d87e7290fb', 'stock_adjustment', '盤點調整單', 'document', true, 'table',
     'stock_adjustments', NULL, 'ADJ{yyyyMM}-{0000}', '{record_no}', 100),
    ('01a1ea12-f322-7001-a093-e320da8dffef', 'stock_adjustment_line', '盤點調整明細', 'detail', true, 'table',
     'stock_adjustment_lines', '01a1ea12-f322-7000-8bce-42d87e7290fb', NULL,
     '{product_id.code} {product_id.name} {lot_no}', 0);

-- ============================================================================
-- 系統欄位（sort_order 依表中順序 10、20、30…）
-- ============================================================================

-- stock_adjustment（status 為系統維護：唯讀、不可必填、沒有預設值，§7.1）
INSERT INTO field_definitions
    (id, entity_id, field_key, label, data_type, is_system, storage, column_name,
     is_required, is_readonly, default_value, validation, options,
     decimal_places, relation_entity_id, sort_order)
VALUES
    ('01a1ea12-f322-7002-8256-c4c17a6c7f3e', '01a1ea12-f322-7000-8bce-42d87e7290fb', 'adjustment_date', '調整日期', 'date', true, 'column', 'adjustment_date',
     true, false, NULL, NULL, NULL,
     NULL, NULL, 10),
    ('01a1ea12-f322-7003-bc43-ba6a2b21a4e3', '01a1ea12-f322-7000-8bce-42d87e7290fb', 'adjustment_type', '類型', 'select', true, 'column', 'adjustment_type',
     true, false, '"count"'::jsonb, NULL,
     '[{"value":"opening","label":"期初庫存"},{"value":"count","label":"盤點"},{"value":"adjustment","label":"一般調整"}]'::jsonb,
     NULL, NULL, 20),
    ('01a1ea12-f322-7004-aff8-f4465e283c4a', '01a1ea12-f322-7000-8bce-42d87e7290fb', 'status', '狀態', 'select', true, 'column', 'status',
     false, true, NULL, NULL,
     '[{"value":"draft","label":"草稿"},{"value":"posted","label":"已過帳"},{"value":"void","label":"已作廢"}]'::jsonb,
     NULL, NULL, 30),
    ('01a1ea12-f322-7005-b483-33111227a9c5', '01a1ea12-f322-7000-8bce-42d87e7290fb', 'notes', '備註', 'long_text', true, 'column', 'notes',
     false, false, NULL, NULL, NULL,
     NULL, NULL, 40);

-- stock_adjustment_line
INSERT INTO field_definitions
    (id, entity_id, field_key, label, data_type, is_system, storage, column_name,
     is_required, is_readonly, default_value, validation, options,
     decimal_places, relation_entity_id, sort_order)
VALUES
    ('01a1ea12-f322-7006-a6e8-2bf589583b37', '01a1ea12-f322-7001-a093-e320da8dffef', 'product_id', '商品', 'relation', true, 'column', 'product_id',
     true, false, NULL, NULL, NULL,
     NULL, '01a1e516-3401-7432-acb3-9cc3ac6a7db9', 10),
    ('01a1ea12-f322-7007-b4c8-45571a37bd88', '01a1ea12-f322-7001-a093-e320da8dffef', 'line_type', '調整方式', 'select', true, 'column', 'line_type',
     true, false, '"gain_new"'::jsonb, NULL,
     '[{"value":"gain_new","label":"盤盈：新批號"},{"value":"gain_existing","label":"盤盈：加回既有批號"},{"value":"loss","label":"盤虧：扣指定批號"}]'::jsonb,
     NULL, NULL, 20),
    ('01a1ea12-f322-7008-bece-12711ac6c9d8', '01a1ea12-f322-7001-a093-e320da8dffef', 'lot_no', '批號', 'text', true, 'column', 'lot_no',
     false, false, NULL, '{"max_length":50}'::jsonb, NULL,
     NULL, NULL, 30),
    ('01a1ea12-f322-7009-8da0-0482c3c3b89e', '01a1ea12-f322-7001-a093-e320da8dffef', 'qty', '數量', 'decimal', true, 'column', 'qty',
     true, false, NULL, '{"min":"0"}'::jsonb, NULL,
     4, NULL, 40),
    ('01a1ea12-f322-700a-a45c-d672ac2dba30', '01a1ea12-f322-7001-a093-e320da8dffef', 'unit_cost_twd', '單位成本', 'money', true, 'column', 'unit_cost_twd',
     false, false, NULL, '{"min":"0"}'::jsonb, NULL,
     4, NULL, 50),
    ('01a1ea12-f322-700b-8f52-65a501f620b0', '01a1ea12-f322-7001-a093-e320da8dffef', 'reason', '原因', 'text', true, 'column', 'reason',
     false, false, NULL, NULL, NULL,
     NULL, NULL, 60);

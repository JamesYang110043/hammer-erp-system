-- 0010_master_data：M3 基本資料表與系統資料類別（#105，docs/architecture.md §7.1、§7.2、§8.6、§8.7）。
--
-- 建立 units、product_categories、products、currencies、exchange_rates、tax_rates、
-- customers、customer_contacts、suppliers、supplier_contacts，寫入預設幣別與稅別，
-- 並把 7 個系統資料類別與其系統欄位登記到 entity_definitions、field_definitions。
--
-- 系統資料表（units 到 supplier_contacts，不含幣別、匯率、稅別）依 §8.6「系統資料表的共同規則」：
-- 系統欄位 → custom_data → formula_errors → 共通欄位；文字欄位沒有值時存 NULL。
-- 「唯一」的 code 以 lower(code) 建部分唯一索引（不分大小寫、只限未刪除的資料）；
-- 「可搜尋」的欄位建 lower(<欄位>) 索引。
--
-- 資料類別、欄位定義、幣別的 id 為固定的 uuid v7，方便跨環境比對。
-- 欄位定義必須與 architecture.md §8.6、§8.7 一致，由 crates/db/tests/master_data.rs 檢查。

-- ============================================================================
-- currencies（不是資料類別；幣別不可刪除，所以沒有 deleted_at）
-- ============================================================================

CREATE TABLE currencies (
    id              UUID PRIMARY KEY,
    code            TEXT NOT NULL UNIQUE CHECK (code ~ '^[A-Z]{3}$'),
    name            TEXT NOT NULL,
    symbol          TEXT,
    decimal_places  SMALLINT NOT NULL CHECK (decimal_places BETWEEN 0 AND 4),
    is_base         BOOLEAN NOT NULL DEFAULT false,
    is_active       BOOLEAN NOT NULL DEFAULT true,
    -- 共通欄位（§8.1），不含 deleted_at
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by      UUID REFERENCES users(id),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by      UUID REFERENCES users(id),
    version         INTEGER NOT NULL DEFAULT 1
);

COMMENT ON TABLE currencies IS '幣別（§8.6）；不可刪除，只能停用';
COMMENT ON COLUMN currencies.code IS 'ISO 4217 代碼，3 個大寫英文字母；建立後不可修改，其他資料表以 code 參照';
COMMENT ON COLUMN currencies.decimal_places IS '金額的小數位數，0–4';
COMMENT ON COLUMN currencies.is_base IS '本位幣，只有 TWD 為 true';

-- 本位幣只能有一個
CREATE UNIQUE INDEX uq_currencies_is_base ON currencies (is_base) WHERE is_base;

CREATE TRIGGER trg_currencies_set_updated_at
    BEFORE UPDATE ON currencies
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- exchange_rates
-- ============================================================================

CREATE TABLE exchange_rates (
    id             UUID PRIMARY KEY,
    currency_code  TEXT NOT NULL REFERENCES currencies(code),
    rate_date      DATE NOT NULL,
    rate           NUMERIC(18,6) NOT NULL CHECK (rate > 0),
    -- 共通欄位（§8.1）
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by     UUID REFERENCES users(id),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by     UUID REFERENCES users(id),
    deleted_at     TIMESTAMPTZ,
    version        INTEGER NOT NULL DEFAULT 1
);

COMMENT ON TABLE exchange_rates IS '匯率（§8.6）；幣別不可為本位幣，由 hammer_core::exchange_rates 檢查';
COMMENT ON COLUMN exchange_rates.rate_date IS '匯率適用的日期（Asia/Taipei）';
COMMENT ON COLUMN exchange_rates.rate IS '1 外幣 = ? 台幣';

-- 同一幣別同一天只有一筆未刪除的匯率；也用於查詢適用的匯率
CREATE UNIQUE INDEX uq_exchange_rates_currency_date
    ON exchange_rates (currency_code, rate_date) WHERE deleted_at IS NULL;

CREATE TRIGGER trg_exchange_rates_set_updated_at
    BEFORE UPDATE ON exchange_rates
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- tax_rates（由 migration 寫入，M3 不提供修改）
-- ============================================================================

CREATE TABLE tax_rates (
    code  TEXT PRIMARY KEY CHECK (code IN ('taxable', 'zero_rated', 'exempt')),
    name  TEXT NOT NULL,
    rate  NUMERIC(6,4) NOT NULL CHECK (rate >= 0)
);

COMMENT ON TABLE tax_rates IS '稅別（§8.6）；與商品的 tax_type 選項相同';

-- ============================================================================
-- units
-- ============================================================================

CREATE TABLE units (
    id              UUID PRIMARY KEY,
    code            TEXT NOT NULL,
    name            TEXT NOT NULL,
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

COMMENT ON TABLE units IS '單位（§8.6，系統資料類別 unit）';

CREATE UNIQUE INDEX uq_units_code ON units (lower(code)) WHERE deleted_at IS NULL;
CREATE INDEX idx_units_name ON units (lower(name));
CREATE INDEX idx_units_custom_data ON units USING GIN (custom_data);

CREATE TRIGGER trg_units_set_updated_at
    BEFORE UPDATE ON units
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- product_categories
-- ============================================================================

CREATE TABLE product_categories (
    id              UUID PRIMARY KEY,
    name            TEXT NOT NULL,
    parent_id       UUID REFERENCES product_categories(id),
    sort_order      INTEGER NOT NULL DEFAULT 0,
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

COMMENT ON TABLE product_categories IS '商品分類（§8.6，系統資料類別 product_category）；支援多層';
COMMENT ON COLUMN product_categories.parent_id IS '上層分類；不可形成循環，由 hammer_core 檢查';
COMMENT ON COLUMN product_categories.sort_order IS '同一層分類的顯示順序';

CREATE INDEX idx_product_categories_name ON product_categories (lower(name));
CREATE INDEX idx_product_categories_parent_id ON product_categories (parent_id);
CREATE INDEX idx_product_categories_custom_data ON product_categories USING GIN (custom_data);

CREATE TRIGGER trg_product_categories_set_updated_at
    BEFORE UPDATE ON product_categories
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- products
-- ============================================================================

CREATE TABLE products (
    id                  UUID PRIMARY KEY,
    code                TEXT NOT NULL,
    name                TEXT NOT NULL,
    spec                TEXT,
    product_type        TEXT NOT NULL DEFAULT 'goods' CHECK (product_type IN ('goods', 'service')),
    category_id         UUID REFERENCES product_categories(id),
    unit_id             UUID NOT NULL REFERENCES units(id),
    barcode             TEXT,
    tax_type            TEXT NOT NULL DEFAULT 'taxable'
                            CHECK (tax_type IN ('taxable', 'zero_rated', 'exempt')),
    default_sale_price  NUMERIC(18,4) CHECK (default_sale_price >= 0),
    safety_stock        NUMERIC(18,4) CHECK (safety_stock >= 0),
    is_active           BOOLEAN NOT NULL DEFAULT true,
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

COMMENT ON TABLE products IS '商品（§8.6，系統資料類別 product）';
COMMENT ON COLUMN products.product_type IS 'goods（實體商品，有批號和庫存）/ service（服務，無庫存）';
COMMENT ON COLUMN products.tax_type IS 'taxable / zero_rated / exempt，與 tax_rates.code 相同';
COMMENT ON COLUMN products.default_sale_price IS '預設售價（台幣）';

CREATE UNIQUE INDEX uq_products_code ON products (lower(code)) WHERE deleted_at IS NULL;
CREATE INDEX idx_products_name ON products (lower(name));
CREATE INDEX idx_products_barcode ON products (lower(barcode));
CREATE INDEX idx_products_category_id ON products (category_id);
CREATE INDEX idx_products_unit_id ON products (unit_id);
CREATE INDEX idx_products_custom_data ON products USING GIN (custom_data);

CREATE TRIGGER trg_products_set_updated_at
    BEFORE UPDATE ON products
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- customers
-- ============================================================================

CREATE TABLE customers (
    id                  UUID PRIMARY KEY,
    code                TEXT NOT NULL,
    name                TEXT NOT NULL,
    short_name          TEXT,
    tax_id              TEXT,
    phone               TEXT,
    fax                 TEXT,
    email               TEXT,
    address             TEXT,
    invoice_title       TEXT,
    invoice_address     TEXT,
    payment_terms_days  INTEGER CHECK (payment_terms_days >= 0),
    credit_limit        NUMERIC(18,2) CHECK (credit_limit >= 0),
    is_active           BOOLEAN NOT NULL DEFAULT true,
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

COMMENT ON TABLE customers IS '客戶（§8.7，系統資料類別 customer）';
COMMENT ON COLUMN customers.payment_terms_days IS '付款（月結）天數';
COMMENT ON COLUMN customers.credit_limit IS '信用額度（台幣）';

CREATE UNIQUE INDEX uq_customers_code ON customers (lower(code)) WHERE deleted_at IS NULL;
CREATE INDEX idx_customers_name ON customers (lower(name));
CREATE INDEX idx_customers_short_name ON customers (lower(short_name));
CREATE INDEX idx_customers_tax_id ON customers (lower(tax_id));
CREATE INDEX idx_customers_custom_data ON customers USING GIN (custom_data);

CREATE TRIGGER trg_customers_set_updated_at
    BEFORE UPDATE ON customers
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- customer_contacts（明細子表 customer_contact）
-- ============================================================================

CREATE TABLE customer_contacts (
    id              UUID PRIMARY KEY,
    customer_id     UUID NOT NULL REFERENCES customers(id),
    sort_order      INTEGER NOT NULL DEFAULT 0,
    name            TEXT NOT NULL,
    title           TEXT,
    phone           TEXT,
    mobile          TEXT,
    email           TEXT,
    is_primary      BOOLEAN NOT NULL DEFAULT false,
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

COMMENT ON TABLE customer_contacts IS '客戶聯絡人（§8.7，明細子表 customer_contact）';
COMMENT ON COLUMN customer_contacts.customer_id IS '所屬客戶（不是欄位）';
COMMENT ON COLUMN customer_contacts.sort_order IS '明細順序，從 0 開始（不是欄位）';
COMMENT ON COLUMN customer_contacts.is_primary IS '主要聯絡人；每個客戶最多一位，由 hammer_core 檢查';

CREATE INDEX idx_customer_contacts_customer_id
    ON customer_contacts (customer_id, sort_order, id) WHERE deleted_at IS NULL;
CREATE INDEX idx_customer_contacts_custom_data ON customer_contacts USING GIN (custom_data);

CREATE TRIGGER trg_customer_contacts_set_updated_at
    BEFORE UPDATE ON customer_contacts
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- suppliers
-- ============================================================================

CREATE TABLE suppliers (
    id                     UUID PRIMARY KEY,
    code                   TEXT NOT NULL,
    name                   TEXT NOT NULL,
    short_name             TEXT,
    tax_id                 TEXT,
    phone                  TEXT,
    fax                    TEXT,
    email                  TEXT,
    address                TEXT,
    invoice_title          TEXT,
    invoice_address        TEXT,
    payment_terms_days     INTEGER CHECK (payment_terms_days >= 0),
    credit_limit           NUMERIC(18,2) CHECK (credit_limit >= 0),
    default_currency_code  TEXT NOT NULL DEFAULT 'TWD' REFERENCES currencies(code),
    country                TEXT,
    bank_info              TEXT,
    is_active              BOOLEAN NOT NULL DEFAULT true,
    notes                  TEXT,
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

COMMENT ON TABLE suppliers IS '廠商（§8.7，系統資料類別 supplier）';
COMMENT ON COLUMN suppliers.default_currency_code IS '預設幣別；必須是有效的幣別，由 hammer_core 檢查';

CREATE UNIQUE INDEX uq_suppliers_code ON suppliers (lower(code)) WHERE deleted_at IS NULL;
CREATE INDEX idx_suppliers_name ON suppliers (lower(name));
CREATE INDEX idx_suppliers_short_name ON suppliers (lower(short_name));
CREATE INDEX idx_suppliers_tax_id ON suppliers (lower(tax_id));
CREATE INDEX idx_suppliers_default_currency_code ON suppliers (default_currency_code);
CREATE INDEX idx_suppliers_custom_data ON suppliers USING GIN (custom_data);

CREATE TRIGGER trg_suppliers_set_updated_at
    BEFORE UPDATE ON suppliers
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- supplier_contacts（明細子表 supplier_contact）
-- ============================================================================

CREATE TABLE supplier_contacts (
    id              UUID PRIMARY KEY,
    supplier_id     UUID NOT NULL REFERENCES suppliers(id),
    sort_order      INTEGER NOT NULL DEFAULT 0,
    name            TEXT NOT NULL,
    title           TEXT,
    phone           TEXT,
    mobile          TEXT,
    email           TEXT,
    is_primary      BOOLEAN NOT NULL DEFAULT false,
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

COMMENT ON TABLE supplier_contacts IS '廠商聯絡人（§8.7，明細子表 supplier_contact）';
COMMENT ON COLUMN supplier_contacts.supplier_id IS '所屬廠商（不是欄位）';
COMMENT ON COLUMN supplier_contacts.sort_order IS '明細順序，從 0 開始（不是欄位）';
COMMENT ON COLUMN supplier_contacts.is_primary IS '主要聯絡人；每個廠商最多一位，由 hammer_core 檢查';

CREATE INDEX idx_supplier_contacts_supplier_id
    ON supplier_contacts (supplier_id, sort_order, id) WHERE deleted_at IS NULL;
CREATE INDEX idx_supplier_contacts_custom_data ON supplier_contacts USING GIN (custom_data);

CREATE TRIGGER trg_supplier_contacts_set_updated_at
    BEFORE UPDATE ON supplier_contacts
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- 初始資料：幣別、稅別
-- ============================================================================

INSERT INTO currencies (id, code, name, symbol, decimal_places, is_base) VALUES
    ('01a1e516-3446-7db4-b4db-d89e92c4f679', 'TWD', '新台幣', 'NT$', 0, true),
    ('01a1e516-3447-79e3-9b8b-bb266700e028', 'USD', '美元', 'US$', 2, false),
    ('01a1e516-3448-7815-a1d4-37474436ce40', 'JPY', '日圓', 'JP¥', 0, false),
    ('01a1e516-3449-7783-b38d-6a79f2f6e125', 'EUR', '歐元', '€', 2, false),
    ('01a1e516-344a-7a98-9c9f-590657e4db07', 'CNY', '人民幣', 'CN¥', 2, false),
    ('01a1e516-344b-795f-b7a4-b077635e2ab8', 'HKD', '港幣', 'HK$', 2, false);

INSERT INTO tax_rates (code, name, rate) VALUES
    ('taxable',    '應稅',   0.0500),
    ('zero_rated', '零稅率', 0),
    ('exempt',     '免稅',   0);

-- ============================================================================
-- 系統資料類別（§8.6「M3 的系統資料類別」）
-- ============================================================================

INSERT INTO entity_definitions
    (id, key, label, kind, is_system, storage, table_name, parent_entity_id, title_template, sort_order)
VALUES
    ('01a1e516-3401-7432-acb3-9cc3ac6a7db9', 'product', '商品', 'master', true, 'table', 'products', NULL, '{code} {name}', 10),
    ('01a1e516-3402-7dda-9787-42df16dbf8ed', 'product_category', '商品分類', 'master', true, 'table', 'product_categories', NULL, '{name}', 20),
    ('01a1e516-3403-79cd-a852-2ea76c2aad8f', 'unit', '單位', 'master', true, 'table', 'units', NULL, '{name}', 30),
    ('01a1e516-3404-7153-bc28-b536c04db4db', 'customer', '客戶', 'master', true, 'table', 'customers', NULL, '{code} {name}', 40),
    ('01a1e516-3405-74a4-8341-9c565e9e4f18', 'customer_contact', '客戶聯絡人', 'detail', true, 'table', 'customer_contacts', '01a1e516-3404-7153-bc28-b536c04db4db', '{name}', 0),
    ('01a1e516-3406-71fc-8cf2-9ce47caf0e68', 'supplier', '廠商', 'master', true, 'table', 'suppliers', NULL, '{code} {name}', 50),
    ('01a1e516-3407-70ea-8e15-a11f08ad0653', 'supplier_contact', '廠商聯絡人', 'detail', true, 'table', 'supplier_contacts', '01a1e516-3406-71fc-8cf2-9ce47caf0e68', '{name}', 0);

-- ============================================================================
-- 系統欄位（§8.6、§8.7；sort_order 依表中順序 10、20、30…）
-- ============================================================================

-- product
INSERT INTO field_definitions
    (id, entity_id, field_key, label, data_type, is_system, storage, column_name,
     is_required, is_searchable, is_unique, default_value, validation, options,
     decimal_places, relation_entity_id, ui_widget, sort_order)
VALUES
    ('01a1e516-3408-7d88-8d0b-0a4fb3d0ed2b', '01a1e516-3401-7432-acb3-9cc3ac6a7db9', 'code', '料號', 'text', true, 'column', 'code',
     true, false, true, NULL, '{"max_length":50}'::jsonb, NULL,
     NULL, NULL, NULL, 10),
    ('01a1e516-3409-748e-a35e-41242cede8f0', '01a1e516-3401-7432-acb3-9cc3ac6a7db9', 'name', '品名', 'text', true, 'column', 'name',
     true, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 20),
    ('01a1e516-340a-792e-872e-d1c9b51312a6', '01a1e516-3401-7432-acb3-9cc3ac6a7db9', 'spec', '規格', 'text', true, 'column', 'spec',
     false, false, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 30),
    ('01a1e516-340b-7ccd-8562-e47b44bb0d0b', '01a1e516-3401-7432-acb3-9cc3ac6a7db9', 'product_type', '類型', 'select', true, 'column', 'product_type',
     true, false, false, '"goods"'::jsonb, NULL, '[{"value":"goods","label":"實體商品"},{"value":"service","label":"服務"}]'::jsonb,
     NULL, NULL, NULL, 40),
    ('01a1e516-340c-7b71-894a-e26701d2b908', '01a1e516-3401-7432-acb3-9cc3ac6a7db9', 'category_id', '分類', 'relation', true, 'column', 'category_id',
     false, false, false, NULL, NULL, NULL,
     NULL, '01a1e516-3402-7dda-9787-42df16dbf8ed', NULL, 50),
    ('01a1e516-340d-7ab7-9eb0-e7aed2cd1c4c', '01a1e516-3401-7432-acb3-9cc3ac6a7db9', 'unit_id', '單位', 'relation', true, 'column', 'unit_id',
     true, false, false, NULL, NULL, NULL,
     NULL, '01a1e516-3403-79cd-a852-2ea76c2aad8f', NULL, 60),
    ('01a1e516-340e-7a27-9f93-901e23e21ad3', '01a1e516-3401-7432-acb3-9cc3ac6a7db9', 'barcode', '條碼', 'text', true, 'column', 'barcode',
     false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 70),
    ('01a1e516-340f-7ef7-9081-93bc6b3c7fba', '01a1e516-3401-7432-acb3-9cc3ac6a7db9', 'tax_type', '稅別', 'select', true, 'column', 'tax_type',
     true, false, false, '"taxable"'::jsonb, NULL, '[{"value":"taxable","label":"應稅"},{"value":"zero_rated","label":"零稅率"},{"value":"exempt","label":"免稅"}]'::jsonb,
     NULL, NULL, NULL, 80),
    ('01a1e516-3410-7f06-93a7-140f2969126b', '01a1e516-3401-7432-acb3-9cc3ac6a7db9', 'default_sale_price', '預設售價', 'money', true, 'column', 'default_sale_price',
     false, false, false, NULL, '{"min":"0"}'::jsonb, NULL,
     4, NULL, NULL, 90),
    ('01a1e516-3411-71da-85ce-cc7afa987af1', '01a1e516-3401-7432-acb3-9cc3ac6a7db9', 'safety_stock', '安全庫存', 'decimal', true, 'column', 'safety_stock',
     false, false, false, NULL, '{"min":"0"}'::jsonb, NULL,
     4, NULL, NULL, 100),
    ('01a1e516-3412-70ac-83b0-9d98c31399ce', '01a1e516-3401-7432-acb3-9cc3ac6a7db9', 'is_active', '啟用', 'boolean', true, 'column', 'is_active',
     true, false, false, 'true'::jsonb, NULL, NULL,
     NULL, NULL, NULL, 110),
    ('01a1e516-3413-7445-b11f-2cc58e982135', '01a1e516-3401-7432-acb3-9cc3ac6a7db9', 'notes', '備註', 'long_text', true, 'column', 'notes',
     false, false, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 120);

-- product_category
INSERT INTO field_definitions
    (id, entity_id, field_key, label, data_type, is_system, storage, column_name,
     is_required, is_searchable, is_unique, default_value, validation, options,
     decimal_places, relation_entity_id, ui_widget, sort_order)
VALUES
    ('01a1e516-3414-7eeb-ae18-19df11ae7245', '01a1e516-3402-7dda-9787-42df16dbf8ed', 'name', '名稱', 'text', true, 'column', 'name',
     true, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 10),
    ('01a1e516-3415-7afa-b2fa-06208c1b10a3', '01a1e516-3402-7dda-9787-42df16dbf8ed', 'parent_id', '上層分類', 'relation', true, 'column', 'parent_id',
     false, false, false, NULL, NULL, NULL,
     NULL, '01a1e516-3402-7dda-9787-42df16dbf8ed', NULL, 20),
    ('01a1e516-3416-7977-8db7-424983427a2d', '01a1e516-3402-7dda-9787-42df16dbf8ed', 'sort_order', '排序', 'integer', true, 'column', 'sort_order',
     true, false, false, '0'::jsonb, NULL, NULL,
     NULL, NULL, NULL, 30);

-- unit
INSERT INTO field_definitions
    (id, entity_id, field_key, label, data_type, is_system, storage, column_name,
     is_required, is_searchable, is_unique, default_value, validation, options,
     decimal_places, relation_entity_id, ui_widget, sort_order)
VALUES
    ('01a1e516-3417-7bcb-b6f4-10284c50cfc4', '01a1e516-3403-79cd-a852-2ea76c2aad8f', 'code', '代碼', 'text', true, 'column', 'code',
     true, false, true, NULL, '{"max_length":50}'::jsonb, NULL,
     NULL, NULL, NULL, 10),
    ('01a1e516-3418-7983-b5e5-7aca9f50ba45', '01a1e516-3403-79cd-a852-2ea76c2aad8f', 'name', '名稱', 'text', true, 'column', 'name',
     true, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 20);

-- customer
INSERT INTO field_definitions
    (id, entity_id, field_key, label, data_type, is_system, storage, column_name,
     is_required, is_searchable, is_unique, default_value, validation, options,
     decimal_places, relation_entity_id, ui_widget, sort_order)
VALUES
    ('01a1e516-3419-7ee4-9084-dc2a91a20d0b', '01a1e516-3404-7153-bc28-b536c04db4db', 'code', '客戶編號', 'text', true, 'column', 'code',
     true, false, true, NULL, '{"max_length":50}'::jsonb, NULL,
     NULL, NULL, NULL, 10),
    ('01a1e516-341a-7788-bb07-8cd204bfefcf', '01a1e516-3404-7153-bc28-b536c04db4db', 'name', '公司名稱', 'text', true, 'column', 'name',
     true, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 20),
    ('01a1e516-341b-7e5d-afb2-36cd6e4dc880', '01a1e516-3404-7153-bc28-b536c04db4db', 'short_name', '簡稱', 'text', true, 'column', 'short_name',
     false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 30),
    ('01a1e516-341c-7866-b22e-ba8b5f63b9d6', '01a1e516-3404-7153-bc28-b536c04db4db', 'tax_id', '統一編號', 'text', true, 'column', 'tax_id',
     false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 40),
    ('01a1e516-341d-7778-ae95-7d8701103c4d', '01a1e516-3404-7153-bc28-b536c04db4db', 'phone', '電話', 'text', true, 'column', 'phone',
     false, false, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 50),
    ('01a1e516-341e-75b6-bb3c-daaca13f4848', '01a1e516-3404-7153-bc28-b536c04db4db', 'fax', '傳真', 'text', true, 'column', 'fax',
     false, false, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 60),
    ('01a1e516-341f-7b29-be50-33a2ed0ab0e4', '01a1e516-3404-7153-bc28-b536c04db4db', 'email', 'Email', 'text', true, 'column', 'email',
     false, false, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 70),
    ('01a1e516-3420-7a6d-a26b-d2becaa0f7e0', '01a1e516-3404-7153-bc28-b536c04db4db', 'address', '聯絡地址', 'text', true, 'column', 'address',
     false, false, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 80),
    ('01a1e516-3421-7814-b4bf-e89449e8b57c', '01a1e516-3404-7153-bc28-b536c04db4db', 'invoice_title', '發票抬頭', 'text', true, 'column', 'invoice_title',
     false, false, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 90),
    ('01a1e516-3422-7318-9790-b338f27423c9', '01a1e516-3404-7153-bc28-b536c04db4db', 'invoice_address', '發票地址', 'text', true, 'column', 'invoice_address',
     false, false, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 100),
    ('01a1e516-3423-7685-85c1-f57affedf4e4', '01a1e516-3404-7153-bc28-b536c04db4db', 'payment_terms_days', '付款天數', 'integer', true, 'column', 'payment_terms_days',
     false, false, false, NULL, '{"min":"0"}'::jsonb, NULL,
     NULL, NULL, NULL, 110),
    ('01a1e516-3424-7951-8886-e0968d47d9de', '01a1e516-3404-7153-bc28-b536c04db4db', 'credit_limit', '信用額度', 'money', true, 'column', 'credit_limit',
     false, false, false, NULL, '{"min":"0"}'::jsonb, NULL,
     2, NULL, NULL, 120),
    ('01a1e516-3425-710f-a805-51714cad70f8', '01a1e516-3404-7153-bc28-b536c04db4db', 'is_active', '啟用', 'boolean', true, 'column', 'is_active',
     true, false, false, 'true'::jsonb, NULL, NULL,
     NULL, NULL, NULL, 130),
    ('01a1e516-3426-706f-88c6-8b9f12a9902f', '01a1e516-3404-7153-bc28-b536c04db4db', 'notes', '備註', 'long_text', true, 'column', 'notes',
     false, false, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 140);

-- customer_contact
INSERT INTO field_definitions
    (id, entity_id, field_key, label, data_type, is_system, storage, column_name,
     is_required, is_searchable, is_unique, default_value, validation, options,
     decimal_places, relation_entity_id, ui_widget, sort_order)
VALUES
    ('01a1e516-3427-7a65-8453-bcbe92f70fcc', '01a1e516-3405-74a4-8341-9c565e9e4f18', 'name', '姓名', 'text', true, 'column', 'name',
     true, false, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 10),
    ('01a1e516-3428-7514-9947-815af03d802b', '01a1e516-3405-74a4-8341-9c565e9e4f18', 'title', '職稱', 'text', true, 'column', 'title',
     false, false, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 20),
    ('01a1e516-3429-763c-8a14-0fe4b4760567', '01a1e516-3405-74a4-8341-9c565e9e4f18', 'phone', '電話', 'text', true, 'column', 'phone',
     false, false, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 30),
    ('01a1e516-342a-73e9-8606-bbe5bc9c1e32', '01a1e516-3405-74a4-8341-9c565e9e4f18', 'mobile', '手機', 'text', true, 'column', 'mobile',
     false, false, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 40),
    ('01a1e516-342b-7dc8-818d-c8a90f39f8c2', '01a1e516-3405-74a4-8341-9c565e9e4f18', 'email', 'Email', 'text', true, 'column', 'email',
     false, false, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 50),
    ('01a1e516-342c-762a-856a-9907f1b0eda4', '01a1e516-3405-74a4-8341-9c565e9e4f18', 'is_primary', '主要聯絡人', 'boolean', true, 'column', 'is_primary',
     true, false, false, 'false'::jsonb, NULL, NULL,
     NULL, NULL, NULL, 60),
    ('01a1e516-342d-795b-bffe-d3c6bf92cdcd', '01a1e516-3405-74a4-8341-9c565e9e4f18', 'notes', '備註', 'long_text', true, 'column', 'notes',
     false, false, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 70);

-- supplier
INSERT INTO field_definitions
    (id, entity_id, field_key, label, data_type, is_system, storage, column_name,
     is_required, is_searchable, is_unique, default_value, validation, options,
     decimal_places, relation_entity_id, ui_widget, sort_order)
VALUES
    ('01a1e516-342e-7c6e-8cde-2fdfb45d991a', '01a1e516-3406-71fc-8cf2-9ce47caf0e68', 'code', '廠商編號', 'text', true, 'column', 'code',
     true, false, true, NULL, '{"max_length":50}'::jsonb, NULL,
     NULL, NULL, NULL, 10),
    ('01a1e516-342f-75e9-8825-75497e58dbbb', '01a1e516-3406-71fc-8cf2-9ce47caf0e68', 'name', '公司名稱', 'text', true, 'column', 'name',
     true, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 20),
    ('01a1e516-3430-75a7-a560-4e0736c102a9', '01a1e516-3406-71fc-8cf2-9ce47caf0e68', 'short_name', '簡稱', 'text', true, 'column', 'short_name',
     false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 30),
    ('01a1e516-3431-7266-ada6-207961100b65', '01a1e516-3406-71fc-8cf2-9ce47caf0e68', 'tax_id', '統一編號', 'text', true, 'column', 'tax_id',
     false, true, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 40),
    ('01a1e516-3432-7b62-b21b-d01d1001de84', '01a1e516-3406-71fc-8cf2-9ce47caf0e68', 'phone', '電話', 'text', true, 'column', 'phone',
     false, false, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 50),
    ('01a1e516-3433-75c0-8a37-988abfedf73c', '01a1e516-3406-71fc-8cf2-9ce47caf0e68', 'fax', '傳真', 'text', true, 'column', 'fax',
     false, false, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 60),
    ('01a1e516-3434-7874-9b7e-4220598856d5', '01a1e516-3406-71fc-8cf2-9ce47caf0e68', 'email', 'Email', 'text', true, 'column', 'email',
     false, false, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 70),
    ('01a1e516-3435-769d-a136-f3e4a35daa48', '01a1e516-3406-71fc-8cf2-9ce47caf0e68', 'address', '聯絡地址', 'text', true, 'column', 'address',
     false, false, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 80),
    ('01a1e516-3436-71a4-ad5e-6e09cb657556', '01a1e516-3406-71fc-8cf2-9ce47caf0e68', 'invoice_title', '發票抬頭', 'text', true, 'column', 'invoice_title',
     false, false, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 90),
    ('01a1e516-3437-7121-8a8a-e98283ca562a', '01a1e516-3406-71fc-8cf2-9ce47caf0e68', 'invoice_address', '發票地址', 'text', true, 'column', 'invoice_address',
     false, false, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 100),
    ('01a1e516-3438-7a6c-9c5e-4d2c278c9a80', '01a1e516-3406-71fc-8cf2-9ce47caf0e68', 'payment_terms_days', '付款天數', 'integer', true, 'column', 'payment_terms_days',
     false, false, false, NULL, '{"min":"0"}'::jsonb, NULL,
     NULL, NULL, NULL, 110),
    ('01a1e516-3439-7e86-81a7-4842d4d851c1', '01a1e516-3406-71fc-8cf2-9ce47caf0e68', 'credit_limit', '信用額度', 'money', true, 'column', 'credit_limit',
     false, false, false, NULL, '{"min":"0"}'::jsonb, NULL,
     2, NULL, NULL, 120),
    ('01a1e516-343a-7aaa-97cd-05960406eb14', '01a1e516-3406-71fc-8cf2-9ce47caf0e68', 'default_currency_code', '預設幣別', 'text', true, 'column', 'default_currency_code',
     true, false, false, '"TWD"'::jsonb, NULL, NULL,
     NULL, NULL, 'currency', 130),
    ('01a1e516-343b-753d-9238-7e8f64bf42ab', '01a1e516-3406-71fc-8cf2-9ce47caf0e68', 'country', '國家', 'text', true, 'column', 'country',
     false, false, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 140),
    ('01a1e516-343c-7c34-86f2-7020cc2ab761', '01a1e516-3406-71fc-8cf2-9ce47caf0e68', 'bank_info', '匯款資訊', 'long_text', true, 'column', 'bank_info',
     false, false, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 150),
    ('01a1e516-343d-7376-83d0-666dcc866e01', '01a1e516-3406-71fc-8cf2-9ce47caf0e68', 'is_active', '啟用', 'boolean', true, 'column', 'is_active',
     true, false, false, 'true'::jsonb, NULL, NULL,
     NULL, NULL, NULL, 160),
    ('01a1e516-343e-75da-a5f1-d6c8195cbbfd', '01a1e516-3406-71fc-8cf2-9ce47caf0e68', 'notes', '備註', 'long_text', true, 'column', 'notes',
     false, false, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 170);

-- supplier_contact
INSERT INTO field_definitions
    (id, entity_id, field_key, label, data_type, is_system, storage, column_name,
     is_required, is_searchable, is_unique, default_value, validation, options,
     decimal_places, relation_entity_id, ui_widget, sort_order)
VALUES
    ('01a1e516-343f-7623-b1c0-ddcca994bc7a', '01a1e516-3407-70ea-8e15-a11f08ad0653', 'name', '姓名', 'text', true, 'column', 'name',
     true, false, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 10),
    ('01a1e516-3440-7fa5-9281-70ac54515607', '01a1e516-3407-70ea-8e15-a11f08ad0653', 'title', '職稱', 'text', true, 'column', 'title',
     false, false, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 20),
    ('01a1e516-3441-7559-8595-f009b6343f11', '01a1e516-3407-70ea-8e15-a11f08ad0653', 'phone', '電話', 'text', true, 'column', 'phone',
     false, false, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 30),
    ('01a1e516-3442-791d-9acd-d13774b32b57', '01a1e516-3407-70ea-8e15-a11f08ad0653', 'mobile', '手機', 'text', true, 'column', 'mobile',
     false, false, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 40),
    ('01a1e516-3443-72e2-a0a8-1773de0bd315', '01a1e516-3407-70ea-8e15-a11f08ad0653', 'email', 'Email', 'text', true, 'column', 'email',
     false, false, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 50),
    ('01a1e516-3444-7553-bc03-aaf601d4cd6d', '01a1e516-3407-70ea-8e15-a11f08ad0653', 'is_primary', '主要聯絡人', 'boolean', true, 'column', 'is_primary',
     true, false, false, 'false'::jsonb, NULL, NULL,
     NULL, NULL, NULL, 60),
    ('01a1e516-3445-7351-bab4-6d272ff88171', '01a1e516-3407-70ea-8e15-a11f08ad0653', 'notes', '備註', 'long_text', true, 'column', 'notes',
     false, false, false, NULL, NULL, NULL,
     NULL, NULL, NULL, 70);

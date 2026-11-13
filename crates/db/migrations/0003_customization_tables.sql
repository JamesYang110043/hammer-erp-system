-- 0003_customization_tables：客製化模組資料表（docs/architecture.md §7、§8.4、§8.5）。
--
-- 建立 entity_definitions、field_definitions、schema_change_logs、view_layouts、
-- custom_records，以及依賴 entity_definitions 的 number_sequences、attachments。
--
-- 不寫入內建資料類別與欄位定義，隨 M3 各系統資料表一起建立。
-- 欄位設為「可搜尋」「不可重複」時的動態索引（§7.5）由應用程式建立，不在此處。

-- ============================================================================
-- entity_definitions
-- ============================================================================

CREATE TABLE entity_definitions (
    id                 UUID PRIMARY KEY,
    key                TEXT NOT NULL UNIQUE,
    label              TEXT NOT NULL,
    icon               TEXT,
    kind               TEXT NOT NULL CHECK (kind IN ('master', 'document', 'detail')),
    is_system          BOOLEAN NOT NULL DEFAULT false,
    storage            TEXT NOT NULL CHECK (storage IN ('table', 'custom_records')),
    table_name         TEXT,
    parent_entity_id   UUID REFERENCES entity_definitions(id),
    numbering_pattern  TEXT,
    title_template     TEXT,
    sort_order         INTEGER NOT NULL DEFAULT 0,
    is_active          BOOLEAN NOT NULL DEFAULT true,
    -- 共通欄位（§8.1）
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by         UUID REFERENCES users(id),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by         UUID REFERENCES users(id),
    deleted_at         TIMESTAMPTZ,
    version            INTEGER NOT NULL DEFAULT 1,

    CONSTRAINT entity_definitions_table_name_check
        CHECK ((storage = 'table') = (table_name IS NOT NULL)),
    CONSTRAINT entity_definitions_parent_check
        CHECK ((kind = 'detail') = (parent_entity_id IS NOT NULL))
);

COMMENT ON TABLE entity_definitions IS '資料類別定義（§7.1、§8.5）';
COMMENT ON COLUMN entity_definitions.key IS '資料類別代碼，例如 customer、repair_order；建立後不可修改';
COMMENT ON COLUMN entity_definitions.kind IS 'master（主檔）/ document（單據）/ detail（明細子表）';
COMMENT ON COLUMN entity_definitions.storage IS 'table（系統資料表）/ custom_records（通用資料表）';
COMMENT ON COLUMN entity_definitions.table_name IS 'storage = table 時對應的資料表';
COMMENT ON COLUMN entity_definitions.parent_entity_id IS '明細子表所屬的主表';
COMMENT ON COLUMN entity_definitions.numbering_pattern IS '單號規則，例如 SO{yyyyMM}-{0000}';
COMMENT ON COLUMN entity_definitions.title_template IS '資料標題，例如 {record_no} {customer.name}';
COMMENT ON COLUMN entity_definitions.sort_order IS '選單排序';

CREATE INDEX idx_entity_definitions_parent_entity_id ON entity_definitions (parent_entity_id);

CREATE TRIGGER trg_entity_definitions_set_updated_at
    BEFORE UPDATE ON entity_definitions
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- field_definitions
-- ============================================================================

CREATE TABLE field_definitions (
    id                   UUID PRIMARY KEY,
    entity_id            UUID NOT NULL REFERENCES entity_definitions(id),
    field_key            TEXT NOT NULL,
    label                TEXT NOT NULL,
    data_type            TEXT NOT NULL CHECK (data_type IN (
                             'text', 'long_text', 'integer', 'decimal', 'money',
                             'date', 'datetime', 'boolean', 'select', 'multi_select',
                             'relation', 'file', 'formula'
                         )),
    is_system            BOOLEAN NOT NULL DEFAULT false,
    storage              TEXT NOT NULL CHECK (storage IN ('column', 'json')),
    column_name          TEXT,
    is_required          BOOLEAN NOT NULL DEFAULT false,
    is_visible           BOOLEAN NOT NULL DEFAULT true,
    is_readonly          BOOLEAN NOT NULL DEFAULT false,
    is_searchable        BOOLEAN NOT NULL DEFAULT false,
    is_unique            BOOLEAN NOT NULL DEFAULT false,
    default_value        JSONB,
    validation           JSONB,
    options              JSONB,
    decimal_places       SMALLINT CHECK (decimal_places >= 0),
    relation_entity_id   UUID REFERENCES entity_definitions(id),
    formula              TEXT,
    -- formula 的結果不能再是 formula
    formula_result_type  TEXT CHECK (formula_result_type IN (
                             'text', 'long_text', 'integer', 'decimal', 'money',
                             'date', 'datetime', 'boolean'
                         )),
    formula_deps         JSONB,
    ui_widget            TEXT,
    column_width         INTEGER CHECK (column_width > 0),
    help_text            TEXT,
    sort_order           INTEGER NOT NULL DEFAULT 0,
    is_active            BOOLEAN NOT NULL DEFAULT true,
    -- 共通欄位（§8.1）
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by           UUID REFERENCES users(id),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by           UUID REFERENCES users(id),
    deleted_at           TIMESTAMPTZ,
    version              INTEGER NOT NULL DEFAULT 1,

    CONSTRAINT field_definitions_entity_field_key_key UNIQUE (entity_id, field_key),
    CONSTRAINT field_definitions_column_name_check
        CHECK ((storage = 'column') = (column_name IS NOT NULL))
);

COMMENT ON TABLE field_definitions IS '欄位定義（§7.2、§7.3、§8.5）';
COMMENT ON COLUMN field_definitions.field_key IS '欄位代碼，建立後不可修改；也是 JSONB 的鍵與公式的引用名稱';
COMMENT ON COLUMN field_definitions.data_type IS '欄位資料型態，見 §7.3';
COMMENT ON COLUMN field_definitions.storage IS 'column（實體欄位）/ json（custom_data 或 data JSONB）';
COMMENT ON COLUMN field_definitions.column_name IS 'storage = column 時對應的實體欄位';
COMMENT ON COLUMN field_definitions.is_searchable IS '可搜尋，會建立運算式索引（§7.5）';
COMMENT ON COLUMN field_definitions.is_unique IS '不可重複，會建立部分唯一索引（§7.5）';
COMMENT ON COLUMN field_definitions.validation IS '例如 {"min":0,"max":100,"max_length":50,"pattern":"^\\d{8}$"}';
COMMENT ON COLUMN field_definitions.options IS 'select 的選項：[{"value":"a","label":"A","color":"blue"}]';
COMMENT ON COLUMN field_definitions.decimal_places IS 'decimal / money 的小數位數';
COMMENT ON COLUMN field_definitions.relation_entity_id IS 'relation 關聯的資料類別';
COMMENT ON COLUMN field_definitions.formula_deps IS '解析後的相依欄位，供相依圖使用（§7.6）';
COMMENT ON COLUMN field_definitions.is_active IS '自訂欄位只能停用不能刪除，資料保留（§7.2）';

CREATE INDEX idx_field_definitions_relation_entity_id ON field_definitions (relation_entity_id);

CREATE TRIGGER trg_field_definitions_set_updated_at
    BEFORE UPDATE ON field_definitions
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- schema_change_logs
-- ============================================================================

CREATE TABLE schema_change_logs (
    id           UUID PRIMARY KEY,
    changed_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    changed_by   UUID REFERENCES users(id),
    target_type  TEXT NOT NULL CHECK (target_type IN ('entity', 'field')),
    target_id    UUID NOT NULL,
    action       TEXT NOT NULL CHECK (action IN ('create', 'update', 'deactivate')),
    before_data  JSONB,
    after_data   JSONB
);

COMMENT ON TABLE schema_change_logs IS '資料類別與欄位定義的變更紀錄（§8.5）';
COMMENT ON COLUMN schema_change_logs.target_id IS 'entity_definitions.id 或 field_definitions.id，依 target_type';

CREATE INDEX idx_schema_change_logs_target ON schema_change_logs (target_type, target_id);
CREATE INDEX idx_schema_change_logs_changed_at ON schema_change_logs (changed_at);

-- ============================================================================
-- view_layouts
-- ============================================================================

CREATE TABLE view_layouts (
    id          UUID PRIMARY KEY,
    entity_id   UUID NOT NULL REFERENCES entity_definitions(id),
    -- NULL：全域預設，只有管理者能設定
    user_id     UUID REFERENCES users(id),
    name        TEXT NOT NULL,
    config      JSONB NOT NULL DEFAULT '{}',
    is_default  BOOLEAN NOT NULL DEFAULT false,
    -- 共通欄位（§8.1）
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by  UUID REFERENCES users(id),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by  UUID REFERENCES users(id),
    deleted_at  TIMESTAMPTZ,
    version     INTEGER NOT NULL DEFAULT 1
);

COMMENT ON TABLE view_layouts IS '列表版面（§8.5）';
COMMENT ON COLUMN view_layouts.user_id IS 'NULL 代表全域預設（只有管理者能設定）';
COMMENT ON COLUMN view_layouts.config IS '顯示欄位、欄寬、排序、篩選條件';

CREATE INDEX idx_view_layouts_entity_user ON view_layouts (entity_id, user_id);

CREATE TRIGGER trg_view_layouts_set_updated_at
    BEFORE UPDATE ON view_layouts
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- custom_records
-- ============================================================================

CREATE TABLE custom_records (
    id                UUID PRIMARY KEY,
    entity_id         UUID NOT NULL REFERENCES entity_definitions(id),
    parent_record_id  UUID REFERENCES custom_records(id),
    record_no         TEXT,
    data              JSONB NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(data) = 'object'),
    -- 共通欄位（§8.1）
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by        UUID REFERENCES users(id),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by        UUID REFERENCES users(id),
    deleted_at        TIMESTAMPTZ,
    version           INTEGER NOT NULL DEFAULT 1
);

COMMENT ON TABLE custom_records IS '自訂資料類別的資料（§7.1、§8.5）';
COMMENT ON COLUMN custom_records.parent_record_id IS '明細子表指向主表的資料';
COMMENT ON COLUMN custom_records.record_no IS '依單號規則產生（如果有設定）';
COMMENT ON COLUMN custom_records.data IS '所有欄位的值，鍵為 field_key，格式見 §7.3';

CREATE INDEX idx_custom_records_entity_id ON custom_records (entity_id);
CREATE INDEX idx_custom_records_parent_record_id ON custom_records (parent_record_id);
CREATE INDEX idx_custom_records_data ON custom_records USING GIN (data);

CREATE TRIGGER trg_custom_records_set_updated_at
    BEFORE UPDATE ON custom_records
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- number_sequences
-- ============================================================================

CREATE TABLE number_sequences (
    id             UUID PRIMARY KEY,
    entity_id      UUID NOT NULL REFERENCES entity_definitions(id),
    -- 單號規則沒有期間時為空字串
    period_key     TEXT NOT NULL DEFAULT '',
    current_value  INTEGER NOT NULL DEFAULT 0 CHECK (current_value >= 0),

    CONSTRAINT number_sequences_entity_period_key UNIQUE (entity_id, period_key)
);

COMMENT ON TABLE number_sequences IS '單號流水號（§8.4）；用 SELECT … FOR UPDATE 取號';
COMMENT ON COLUMN number_sequences.period_key IS '依單號規則產生，例如 202611；沒有期間時為空字串';

-- ============================================================================
-- attachments
-- ============================================================================

CREATE TABLE attachments (
    id            UUID PRIMARY KEY,
    entity_key    TEXT NOT NULL REFERENCES entity_definitions(key),
    record_id     UUID NOT NULL,
    field_key     TEXT NOT NULL,
    file_name     TEXT NOT NULL,
    mime_type     TEXT NOT NULL,
    size_bytes    BIGINT NOT NULL CHECK (size_bytes >= 0),
    sha256        TEXT NOT NULL,
    storage_path  TEXT NOT NULL UNIQUE,
    -- 共通欄位（§8.1）
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by    UUID REFERENCES users(id),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by    UUID REFERENCES users(id),
    deleted_at    TIMESTAMPTZ,
    version       INTEGER NOT NULL DEFAULT 1
);

COMMENT ON TABLE attachments IS '附件（§8.4）；檔案存在伺服器磁碟';
COMMENT ON COLUMN attachments.entity_key IS '附件所屬資料的資料類別代碼';
COMMENT ON COLUMN attachments.record_id IS '附件所屬的資料；可能是系統資料表或 custom_records，因此不設外鍵';
COMMENT ON COLUMN attachments.field_key IS '附件所屬的 file 欄位';
COMMENT ON COLUMN attachments.storage_path IS '伺服器磁碟上的相對路徑';

CREATE INDEX idx_attachments_record ON attachments (entity_key, record_id);

CREATE TRIGGER trg_attachments_set_updated_at
    BEFORE UPDATE ON attachments
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- 0016_form_layouts：表單版面（#221，docs/architecture.md §7.7、§8.5 `form_layouts`）。
--
-- 每個資料類別（主檔、單據）最多一筆有效的版面，只有全域版面。
-- 還原成預設時軟刪除，之後再設定時建立新的一筆。

CREATE TABLE form_layouts (
    id          UUID PRIMARY KEY,
    entity_id   UUID NOT NULL REFERENCES entity_definitions(id),
    config      JSONB NOT NULL,
    -- 共通欄位（§8.1）
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by  UUID REFERENCES users(id),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by  UUID REFERENCES users(id),
    deleted_at  TIMESTAMPTZ,
    version     INTEGER NOT NULL DEFAULT 1
);

CREATE UNIQUE INDEX uq_form_layouts_entity
    ON form_layouts (entity_id)
    WHERE deleted_at IS NULL;

CREATE TRIGGER trg_form_layouts_set_updated_at
    BEFORE UPDATE ON form_layouts
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

COMMENT ON TABLE form_layouts IS '表單版面（§7.7、§8.5）';
COMMENT ON COLUMN form_layouts.config IS '區塊與項目（欄位、空白格、系統區塊），格式見 §7.7';

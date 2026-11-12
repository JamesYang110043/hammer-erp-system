-- 0001_foundation：資料存取層的共用機制（docs/architecture.md §8.1）。
--
-- 這個 migration 不建立業務資料表，只提供之後所有資料表共用的 trigger function。
--
-- ============================================================================
-- 共通欄位慣例
-- ============================================================================
--
-- 資料表定義中寫「＋共通欄位」的，都要包含以下欄位：
--
--   created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
--   created_by  UUID REFERENCES users(id),
--   updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
--   updated_by  UUID REFERENCES users(id),
--   deleted_at  TIMESTAMPTZ,               -- 軟刪除；NULL 代表未刪除
--   version     INTEGER NOT NULL DEFAULT 1 -- 樂觀鎖定
--
-- 規則：
--   * updated_at 由 trigger 自動更新，應用程式不需要自行設定。每張有
--     updated_at 的資料表都要建立：
--
--       CREATE TRIGGER trg_<table>_set_updated_at
--           BEFORE UPDATE ON <table>
--           FOR EACH ROW EXECUTE FUNCTION set_updated_at();
--
--   * version 由應用程式在 UPDATE 時遞增，並以 WHERE 條件檢查（§9.4）：
--
--       UPDATE <table> SET …, version = version + 1
--       WHERE id = $1 AND version = $2
--
--     影響 0 筆時視為衝突，使用 hammer_db::ensure_version_matched 轉成
--     DbError::Conflict。
--
--   * 軟刪除：設定 deleted_at，不實際 DELETE。查詢時加上 deleted_at IS NULL。
--   * created_by / updated_by 指向 users(id)；users 資料表在 M1 建立，
--     在那之前建立的資料表要等 users 存在後再補上外鍵。
--
-- ============================================================================
-- custom_data 慣例
-- ============================================================================
--
-- 可客製化的資料表另外加上：
--
--   custom_data JSONB NOT NULL DEFAULT '{}'
--
-- 規則：
--   * 內容是 JSON 物件，鍵為 field_definitions.field_key，值依欄位型態序列化
--     （§7.3）。
--   * 只存放 storage = 'json' 的自訂欄位；storage = 'column' 的欄位存在實體欄位。
--   * 需要搜尋或唯一性的欄位，依 §7.5 建立運算式索引。

CREATE FUNCTION set_updated_at() RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    NEW.updated_at := now();
    RETURN NEW;
END;
$$;

COMMENT ON FUNCTION set_updated_at() IS
    '共用 trigger：UPDATE 時把 updated_at 設為目前時間。用法：BEFORE UPDATE ON <table> FOR EACH ROW EXECUTE FUNCTION set_updated_at()';

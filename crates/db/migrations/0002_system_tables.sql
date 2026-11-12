-- 0002_system_tables：系統模組資料表（docs/architecture.md §6、§8.4）。
--
-- 建立 roles、role_permissions、users、sessions、audit_logs、settings，
-- 並寫入內建角色、角色權限對照與系統設定預設值。
--
-- number_sequences、attachments 依賴 entity_definitions，在 M2a 建立。
-- 預設管理者帳號不在 migration 建立，由第一次啟動時的引導流程建立（§11）。
--
-- 角色與權限代碼必須與 hammer_shared::role、hammer_shared::permission 一致，
-- 由 crates/db/tests/system_tables.rs 檢查。

-- ============================================================================
-- roles
-- ============================================================================

CREATE TABLE roles (
    id          UUID PRIMARY KEY,
    code        TEXT NOT NULL UNIQUE,
    name        TEXT NOT NULL,
    is_system   BOOLEAN NOT NULL DEFAULT false
);

COMMENT ON TABLE roles IS '角色（§6.1）';
COMMENT ON COLUMN roles.code IS 'admin / editor / viewer，對應 hammer_shared::role';
COMMENT ON COLUMN roles.is_system IS '內建角色不可刪除';

-- ============================================================================
-- role_permissions
-- ============================================================================

CREATE TABLE role_permissions (
    role_id     UUID NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    permission  TEXT NOT NULL,
    PRIMARY KEY (role_id, permission)
);

COMMENT ON TABLE role_permissions IS '角色擁有的權限（§6.1）';
COMMENT ON COLUMN role_permissions.permission IS '權限代碼，對應 hammer_shared::permission';

-- ============================================================================
-- users
-- ============================================================================

CREATE TABLE users (
    id                    UUID PRIMARY KEY,
    username              TEXT NOT NULL UNIQUE,
    display_name          TEXT NOT NULL,
    password_hash         TEXT NOT NULL,
    role_id               UUID NOT NULL REFERENCES roles(id),
    is_active             BOOLEAN NOT NULL DEFAULT true,
    must_change_password  BOOLEAN NOT NULL DEFAULT false,
    failed_login_count    INTEGER NOT NULL DEFAULT 0 CHECK (failed_login_count >= 0),
    locked_until          TIMESTAMPTZ,
    last_login_at         TIMESTAMPTZ,
    -- 共通欄位（§8.1）
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by            UUID REFERENCES users(id),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by            UUID REFERENCES users(id),
    deleted_at            TIMESTAMPTZ,
    version               INTEGER NOT NULL DEFAULT 1
);

COMMENT ON TABLE users IS '帳號（§8.4）';
COMMENT ON COLUMN users.password_hash IS 'Argon2id 雜湊，不保存明文（§6.2）';
COMMENT ON COLUMN users.is_active IS '停用後無法登入';
COMMENT ON COLUMN users.locked_until IS '連續登入失敗後鎖定到此時間（§6.2）';

CREATE TRIGGER trg_users_set_updated_at
    BEFORE UPDATE ON users
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- sessions
-- ============================================================================

CREATE TABLE sessions (
    id            UUID PRIMARY KEY,
    user_id       UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash    TEXT NOT NULL UNIQUE,
    client_info   TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at    TIMESTAMPTZ NOT NULL,
    revoked_at    TIMESTAMPTZ
);

COMMENT ON TABLE sessions IS '登入 session（§6.2）';
COMMENT ON COLUMN sessions.token_hash IS 'session token 的 SHA-256；不保存 token 本身';
COMMENT ON COLUMN sessions.client_info IS '裝置名稱與客戶端版本';
COMMENT ON COLUMN sessions.revoked_at IS '登出或被強制登出的時間';

CREATE INDEX idx_sessions_user_id ON sessions (user_id);

-- ============================================================================
-- audit_logs
-- ============================================================================

CREATE TABLE audit_logs (
    id           UUID PRIMARY KEY,
    occurred_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- NULL：帳號不存在的登入失敗
    user_id      UUID REFERENCES users(id),
    action       TEXT NOT NULL,
    entity_key   TEXT,
    record_id    UUID,
    before_data  JSONB,
    after_data   JSONB,
    client_info  TEXT
);

COMMENT ON TABLE audit_logs IS '操作紀錄（§8.4）';
COMMENT ON COLUMN audit_logs.user_id IS '操作者；帳號不存在的登入失敗為 NULL';
COMMENT ON COLUMN audit_logs.action IS 'create / update / delete / post / void / login / login_failed / export …';
COMMENT ON COLUMN audit_logs.entity_key IS '資料類別代碼';
COMMENT ON COLUMN audit_logs.before_data IS '變更前，只記錄有變動的欄位';
COMMENT ON COLUMN audit_logs.after_data IS '變更後，只記錄有變動的欄位';

CREATE INDEX idx_audit_logs_occurred_at ON audit_logs (occurred_at);
CREATE INDEX idx_audit_logs_user_id ON audit_logs (user_id);
CREATE INDEX idx_audit_logs_entity_record ON audit_logs (entity_key, record_id);

-- ============================================================================
-- settings
-- ============================================================================

CREATE TABLE settings (
    key         TEXT PRIMARY KEY,
    value       JSONB NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by  UUID REFERENCES users(id)
);

COMMENT ON TABLE settings IS '系統設定（§8.4）';

CREATE TRIGGER trg_settings_set_updated_at
    BEFORE UPDATE ON settings
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================================
-- 初始資料
-- ============================================================================

-- 內建角色。id 為固定的 uuid v7，方便跨環境比對。
INSERT INTO roles (id, code, name, is_system) VALUES
    ('01a1d036-f5bf-7553-b9e4-5f2a31b2fef0', 'admin',  '管理者',   true),
    ('01a1d036-f5c0-7776-a34a-06ed5ea7265f', 'editor', '可編輯者', true),
    ('01a1d036-f5c1-7c01-96cb-2b89e400317a', 'viewer', '唯讀者',   true);

-- 角色與權限對照（§6.1）
INSERT INTO role_permissions (role_id, permission)
SELECT r.id, p.permission
FROM roles r
JOIN (VALUES
    ('admin',  'record.read'),
    ('admin',  'record.write'),
    ('admin',  'record.post'),
    ('admin',  'data.export'),
    ('admin',  'schema.manage'),
    ('admin',  'user.manage'),
    ('admin',  'settings.manage'),
    ('admin',  'audit.read'),
    ('editor', 'record.read'),
    ('editor', 'record.write'),
    ('editor', 'record.post'),
    ('editor', 'data.export'),
    ('viewer', 'record.read'),
    ('viewer', 'data.export')
) AS p (role_code, permission) ON p.role_code = r.code;

-- 安全設定預設值（§6.2）
INSERT INTO settings (key, value) VALUES
    ('security.max_failed_logins',    '5'),
    ('security.lockout_minutes',      '15'),
    ('security.session_idle_minutes', '480');

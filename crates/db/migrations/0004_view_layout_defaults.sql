-- 列表版面（view_layouts，§8.5）「預設版面只有一個」的限制（#51）。
--
-- 每個使用者在同一個資料類別最多一個預設版面；全域版面（user_id 為空）的預設也只有一個。
-- 已軟刪除的版面不計。設定新的預設時，應用程式在同一個 transaction 內先取消原本的預設；
-- 兩個請求同時設定預設時，後送出的一個會違反以下唯一索引。

CREATE UNIQUE INDEX uq_view_layouts_user_default
    ON view_layouts (entity_id, user_id)
    WHERE is_default AND user_id IS NOT NULL AND deleted_at IS NULL;

CREATE UNIQUE INDEX uq_view_layouts_global_default
    ON view_layouts (entity_id)
    WHERE is_default AND user_id IS NULL AND deleted_at IS NULL;

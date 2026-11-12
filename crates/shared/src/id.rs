//! 主鍵 ID（`docs/architecture.md` §5、§8.1）。
//!
//! 所有資料表主鍵都是 UUID v7（依時間排序，對 B-tree 索引友善）。
//! 新 ID 一律透過 [`new_id`] 在應用程式端產生，不使用其他版本的 UUID。
//! JSON 中序列化為標準的連字號字串，例如 `"0190c3a2-…"`。

/// 主鍵型別。
pub type Id = uuid::Uuid;

/// 產生新的 UUID v7 主鍵。
pub fn new_id() -> Id {
    uuid::Uuid::now_v7()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_id_is_v7() {
        assert_eq!(new_id().get_version_num(), 7);
    }

    #[test]
    fn new_ids_are_time_ordered() {
        let ids: Vec<Id> = (0..100).map(|_| new_id()).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted);
    }
}

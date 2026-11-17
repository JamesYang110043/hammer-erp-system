import { ReloadOutlined } from '@ant-design/icons';
import { Alert, Button } from 'antd';

/** 樂觀鎖定衝突（409，docs/architecture.md §9.4）的標題。 */
export const CONFLICT_TITLE = '資料已被其他人修改';

/**
 * 修改或刪除時版本不符的提示，附「重新載入」按鈕。
 *
 * `description` 說明重新載入後會發生什麼事（例如關閉視窗、放棄未存檔的修改）。
 */
export function ConflictAlert({
  description = '請重新載入取得最新的資料後再操作。',
  onReload,
  loading,
}: {
  description?: string;
  onReload: () => void;
  loading?: boolean;
}) {
  return (
    <Alert
      showIcon
      type="warning"
      style={{ marginBottom: 16 }}
      message={CONFLICT_TITLE}
      description={description}
      action={
        <Button icon={<ReloadOutlined />} onClick={onReload} loading={loading}>
          重新載入
        </Button>
      }
    />
  );
}

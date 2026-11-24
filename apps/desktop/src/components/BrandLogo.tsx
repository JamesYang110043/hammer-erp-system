import { Typography } from 'antd';

import logoUrl from '../../src-tauri/icons/128x128.png';

/** 產品名稱（Logo 文字、視窗標題）。 */
export const PRODUCT_NAME = 'Hammer ERP';

/**
 * Logo：品牌圖示（與桌面程式圖示相同，`src-tauri/icons`）＋產品名稱。
 *
 * 主畫面側邊欄（深色底）與登入前畫面的標題列（淺色底）共用；`color` 指定名稱文字顏色，
 * `collapsed` 時只顯示圖示（側邊欄收合）。
 */
export function BrandLogo({ color, collapsed = false }: { color?: string; collapsed?: boolean }) {
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 10, minWidth: 0 }}>
      <img src={logoUrl} alt="" width={28} height={28} style={{ flexShrink: 0, borderRadius: 6 }} />
      {!collapsed && (
        <Typography.Title
          level={4}
          ellipsis
          style={{ margin: 0, color, fontSize: 18, whiteSpace: 'nowrap' }}
        >
          {PRODUCT_NAME}
        </Typography.Title>
      )}
    </div>
  );
}

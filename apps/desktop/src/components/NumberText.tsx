import { Typography } from 'antd';

import { isNegativeNumber } from '../records/display';

/**
 * 數字、金額的顯示：等寬數字（`tabular-nums`），負數為錯誤色。
 *
 * 表格欄位另以 `align: 'right'` 靠右對齊。
 */
export function NumberText({ children }: { children: string }) {
  return isNegativeNumber(children) ? (
    <Typography.Text type="danger" className="num">
      {children}
    </Typography.Text>
  ) : (
    <span className="num">{children}</span>
  );
}

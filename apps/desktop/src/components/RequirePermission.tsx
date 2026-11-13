import { Result } from 'antd';
import type { ReactNode } from 'react';

import { usePermissions } from '../auth/permission';
import type { Permission } from '../auth/permission';

/** 沒有權限時顯示提示，不顯示內容（權限仍以伺服器檢查為準）。未指定權限時直接顯示。 */
export function RequirePermission({
  permission,
  children,
}: {
  permission?: Permission;
  children: ReactNode;
}) {
  const can = usePermissions();
  if (permission && !can(permission)) {
    return <Result status="403" title="沒有權限" subTitle="您沒有使用此功能的權限" />;
  }
  return children;
}

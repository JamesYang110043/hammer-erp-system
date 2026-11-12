import { Badge, Space, Tooltip, Typography } from 'antd';
import type { BadgeProps } from 'antd';

import { errorMessage } from '../api/client';
import { useHealth, useServerUrl } from '../api/queries';
import { CLIENT_VERSION } from '../bindings';

interface Indicator {
  status: BadgeProps['status'];
  text: string;
  detail?: string;
}

/** 底部狀態列：伺服器連線狀態與版本。 */
export function StatusBar() {
  const health = useHealth();
  const serverUrl = useServerUrl();

  let indicator: Indicator;
  if (health.isPending) {
    indicator = { status: 'processing', text: '連線中…' };
  } else if (health.isError) {
    indicator = { status: 'error', text: '未連線', detail: errorMessage(health.error) };
  } else if (health.data.database !== 'ok') {
    indicator = {
      status: 'warning',
      text: '已連線（資料庫異常）',
      detail: '伺服器正常，但無法連線到資料庫',
    };
  } else {
    indicator = { status: 'success', text: '已連線' };
  }

  return (
    <Space size="large" style={{ width: '100%', justifyContent: 'space-between' }}>
      <Tooltip title={indicator.detail}>
        <Badge status={indicator.status} text={indicator.text} />
      </Tooltip>
      <Space size="large">
        {serverUrl.data && <Typography.Text type="secondary">{serverUrl.data}</Typography.Text>}
        <Typography.Text type="secondary">
          {/* 斷線時不顯示上一次取得的版本 */}
          伺服器 {health.data && !health.isError ? `v${health.data.version}` : '—'}
        </Typography.Text>
        <Typography.Text type="secondary">用戶端 v{CLIENT_VERSION}</Typography.Text>
      </Space>
    </Space>
  );
}

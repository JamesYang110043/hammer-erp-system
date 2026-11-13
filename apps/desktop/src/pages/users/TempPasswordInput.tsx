import { SyncOutlined } from '@ant-design/icons';
import { Button, Input, Space, Tooltip } from 'antd';

import { generateTempPassword } from '../../utils/tempPassword';

/**
 * 臨時密碼輸入框：明碼顯示（管理者要交給使用者），可以重新產生。
 *
 * 給 `Form.Item` 使用（受控的 `value` / `onChange`）。
 */
export function TempPasswordInput({
  value,
  onChange,
  id,
}: {
  value?: string;
  onChange?: (value: string) => void;
  id?: string;
}) {
  return (
    <Space.Compact style={{ width: '100%' }}>
      <Input
        id={id}
        value={value}
        onChange={(e) => onChange?.(e.target.value)}
        autoComplete="off"
        spellCheck={false}
      />
      <Tooltip title="重新產生">
        <Button
          aria-label="重新產生密碼"
          icon={<SyncOutlined />}
          onClick={() => onChange?.(generateTempPassword())}
        />
      </Tooltip>
    </Space.Compact>
  );
}

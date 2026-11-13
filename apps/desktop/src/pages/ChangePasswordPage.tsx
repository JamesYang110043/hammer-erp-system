import { Alert, App, Button, Space } from 'antd';

import { useLogout } from '../auth/session';
import { ChangePasswordForm } from '../components/ChangePasswordForm';
import { AuthLayout } from '../layouts/AuthLayout';

/** 強制改密碼：新帳號或管理者重設密碼後第一次登入（docs/architecture.md §6.2）。 */
export function ChangePasswordPage() {
  const { message } = App.useApp();
  const logout = useLogout();

  return (
    <AuthLayout title="修改密碼">
      <Space direction="vertical" size="middle" style={{ width: '100%' }}>
        <Alert showIcon type="info" message="第一次登入或密碼已被重設，請先修改密碼再繼續使用。" />
        <ChangePasswordForm onSuccess={() => message.success('密碼已修改')} />
        <Button type="link" block onClick={() => logout.mutate()} loading={logout.isPending}>
          登出
        </Button>
      </Space>
    </AuthLayout>
  );
}

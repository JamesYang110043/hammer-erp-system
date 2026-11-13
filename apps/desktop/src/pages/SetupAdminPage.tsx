import { Alert, Button, Form, Input, Space, Typography } from 'antd';

import { errorMessage } from '../api/client';
import { passwordRule, sameAsRule } from '../auth/passwordRules';
import { useSetupAdmin } from '../auth/session';
import { AuthLayout } from '../layouts/AuthLayout';

interface FormValues {
  username: string;
  displayName: string;
  password: string;
  confirmPassword: string;
}

/** 第一次啟動：伺服器還沒有任何帳號，建立第一個管理者，完成後直接登入。 */
export function SetupAdminPage() {
  const setup = useSetupAdmin();

  const handleFinish = (values: FormValues) => {
    setup.mutate({
      username: values.username.trim(),
      display_name: values.displayName.trim(),
      password: values.password,
    });
  };

  return (
    <AuthLayout title="建立管理者">
      <Space direction="vertical" size="middle" style={{ width: '100%' }}>
        <Typography.Paragraph type="secondary" style={{ margin: 0 }}>
          系統尚未建立任何帳號。請先建立管理者帳號，之後再由管理者新增其他使用者。
        </Typography.Paragraph>
        <Form layout="vertical" onFinish={handleFinish} disabled={setup.isPending}>
          <Form.Item
            label="帳號"
            name="username"
            rules={[{ required: true, whitespace: true, message: '請輸入帳號' }]}
          >
            <Input autoComplete="username" autoFocus />
          </Form.Item>
          <Form.Item
            label="顯示名稱"
            name="displayName"
            rules={[{ required: true, whitespace: true, message: '請輸入顯示名稱' }]}
          >
            <Input />
          </Form.Item>
          <Form.Item
            label="密碼"
            name="password"
            rules={[{ required: true, message: '請輸入密碼' }, passwordRule]}
          >
            <Input.Password autoComplete="new-password" />
          </Form.Item>
          <Form.Item
            label="確認密碼"
            name="confirmPassword"
            dependencies={['password']}
            rules={[{ required: true, message: '請再輸入一次密碼' }, sameAsRule('password')]}
          >
            <Input.Password autoComplete="new-password" />
          </Form.Item>
          {setup.isError && (
            <Form.Item>
              <Alert showIcon type="error" message={errorMessage(setup.error)} />
            </Form.Item>
          )}
          <Button type="primary" htmlType="submit" block loading={setup.isPending}>
            建立並登入
          </Button>
        </Form>
      </Space>
    </AuthLayout>
  );
}

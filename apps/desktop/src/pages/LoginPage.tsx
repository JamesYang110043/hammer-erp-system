import { Alert, Button, Form, Input, Space } from 'antd';

import { errorMessage } from '../api/client';
import { useLogin } from '../auth/session';
import { AuthLayout } from '../layouts/AuthLayout';

interface FormValues {
  username: string;
  password: string;
}

/**
 * 登入頁。
 *
 * 錯誤訊息直接顯示伺服器的訊息：帳號或密碼錯誤、帳號鎖定（含解鎖時間，台北時間）、帳號停用。
 */
export function LoginPage({ notice }: { notice?: string }) {
  const login = useLogin();

  const handleFinish = (values: FormValues) => {
    login.mutate({ username: values.username.trim(), password: values.password });
  };

  return (
    <AuthLayout title="登入">
      <Space direction="vertical" size="middle" style={{ width: '100%' }}>
        {notice && !login.isError && <Alert showIcon type="warning" message={notice} />}
        <Form layout="vertical" onFinish={handleFinish} disabled={login.isPending}>
          <Form.Item
            label="帳號"
            name="username"
            rules={[{ required: true, whitespace: true, message: '請輸入帳號' }]}
          >
            <Input autoComplete="username" autoFocus />
          </Form.Item>
          <Form.Item
            label="密碼"
            name="password"
            rules={[{ required: true, message: '請輸入密碼' }]}
          >
            <Input.Password autoComplete="current-password" />
          </Form.Item>
          {login.isError && (
            <Form.Item>
              <Alert showIcon type="error" message={errorMessage(login.error)} />
            </Form.Item>
          )}
          <Button type="primary" htmlType="submit" block loading={login.isPending}>
            登入
          </Button>
        </Form>
      </Space>
    </AuthLayout>
  );
}

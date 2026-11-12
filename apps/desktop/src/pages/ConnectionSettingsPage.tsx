import { Alert, App, Button, Card, Form, Input, Space, Spin } from 'antd';
import { useEffect } from 'react';

import { errorMessage } from '../api/client';
import { useSaveServerUrl, useServerUrl, useTestConnection } from '../api/queries';

interface FormValues {
  serverUrl: string;
}

/** 連線設定：修改伺服器網址、測試連線。 */
export function ConnectionSettingsPage() {
  const { message } = App.useApp();
  const [form] = Form.useForm<FormValues>();
  const serverUrl = useServerUrl();
  const test = useTestConnection();
  const save = useSaveServerUrl();

  useEffect(() => {
    if (serverUrl.data !== undefined) {
      form.setFieldsValue({ serverUrl: serverUrl.data });
    }
  }, [form, serverUrl.data]);

  const handleTest = async () => {
    const { serverUrl: url } = await form.validateFields();
    test.mutate(url);
  };

  const handleSave = (values: FormValues) => {
    save.mutate(values.serverUrl, {
      onSuccess: (saved) => {
        form.setFieldsValue({ serverUrl: saved });
        message.success('已儲存伺服器網址');
      },
    });
  };

  if (serverUrl.isPending) {
    return <Spin />;
  }

  return (
    <Card title="連線設定" style={{ maxWidth: 640 }}>
      <Form form={form} layout="vertical" onFinish={handleSave} onValuesChange={() => test.reset()}>
        <Form.Item
          label="伺服器網址"
          name="serverUrl"
          rules={[{ required: true, whitespace: true, message: '請輸入伺服器網址' }]}
          extra="例如：http://localhost:8080"
        >
          <Input placeholder="http://localhost:8080" autoComplete="off" />
        </Form.Item>
        <Form.Item>
          <Space>
            <Button onClick={handleTest} loading={test.isPending}>
              測試連線
            </Button>
            <Button type="primary" htmlType="submit" loading={save.isPending}>
              儲存
            </Button>
          </Space>
        </Form.Item>
      </Form>

      {test.isSuccess && (
        <Alert
          showIcon
          type={test.data.database === 'ok' ? 'success' : 'warning'}
          message={
            test.data.database === 'ok'
              ? `連線成功，伺服器版本 v${test.data.version}`
              : `已連到伺服器（v${test.data.version}），但伺服器無法連線到資料庫`
          }
        />
      )}
      {test.isError && <Alert showIcon type="error" message={errorMessage(test.error)} />}
      {save.isError && <Alert showIcon type="error" message={errorMessage(save.error)} />}
    </Card>
  );
}

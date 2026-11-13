import { ApiOutlined } from '@ant-design/icons';
import { Button, Card, Layout, Modal, Typography, theme } from 'antd';
import { useState } from 'react';
import type { ReactNode } from 'react';

import { StatusBar } from '../components/StatusBar';
import { ConnectionSettingsPage } from '../pages/ConnectionSettingsPage';

const { Header, Content, Footer } = Layout;

/**
 * 登入前的畫面框架（建立管理者、登入、強制改密碼、連線失敗）。
 *
 * 右上角可以開啟連線設定：伺服器網址錯誤時，不用登入也能修改。
 */
export function AuthLayout({ title, children }: { title: string; children: ReactNode }) {
  const { token } = theme.useToken();
  const [settingsOpen, setSettingsOpen] = useState(false);

  return (
    <Layout style={{ height: '100vh' }}>
      <Header
        style={{
          background: token.colorBgContainer,
          borderBottom: `1px solid ${token.colorSplit}`,
          display: 'flex',
          justifyContent: 'space-between',
          alignItems: 'center',
          paddingInline: 24,
        }}
      >
        <Typography.Title level={4} style={{ margin: 0 }}>
          Hammer ERP
        </Typography.Title>
        <Button type="text" icon={<ApiOutlined />} onClick={() => setSettingsOpen(true)}>
          連線設定
        </Button>
      </Header>
      <Content
        style={{
          display: 'flex',
          justifyContent: 'center',
          alignItems: 'flex-start',
          padding: '64px 24px',
          overflow: 'auto',
        }}
      >
        <Card title={title} style={{ width: 400, maxWidth: '100%' }}>
          {children}
        </Card>
      </Content>
      <Footer
        style={{
          padding: '4px 16px',
          background: token.colorBgContainer,
          borderTop: `1px solid ${token.colorSplit}`,
          fontSize: token.fontSizeSM,
        }}
      >
        <StatusBar />
      </Footer>
      <Modal
        open={settingsOpen}
        onCancel={() => setSettingsOpen(false)}
        footer={null}
        width={688}
        destroyOnHidden
      >
        <ConnectionSettingsPage />
      </Modal>
    </Layout>
  );
}

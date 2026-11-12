import { ApiOutlined } from '@ant-design/icons';
import { Button, Layout, Menu, Typography, theme } from 'antd';
import { Outlet, useLocation, useNavigate } from 'react-router';

import { StatusBar } from '../components/StatusBar';
import { CONNECTION_SETTINGS_PATH, MODULES } from '../modules';

const { Header, Sider, Content, Footer } = Layout;

/** 主畫面框架：左側模組選單、上方標題列、中間內容、底部狀態列。 */
export function AppLayout() {
  const navigate = useNavigate();
  const location = useLocation();
  const { token } = theme.useToken();

  // `/customers/123` → `customers`
  const selected = location.pathname.split('/')[1] ?? '';

  return (
    <Layout style={{ height: '100vh' }}>
      <Sider width={200} theme="light" style={{ borderRight: `1px solid ${token.colorSplit}` }}>
        <div style={{ padding: '16px 24px' }}>
          <Typography.Title level={4} style={{ margin: 0 }}>
            Hammer ERP
          </Typography.Title>
        </div>
        <Menu
          mode="inline"
          selectedKeys={[selected]}
          items={MODULES.map((m) => ({ key: m.path, icon: m.icon, label: m.label }))}
          onClick={({ key }) => navigate(`/${key}`)}
          style={{ borderInlineEnd: 'none' }}
        />
      </Sider>
      <Layout>
        <Header
          style={{
            background: token.colorBgContainer,
            borderBottom: `1px solid ${token.colorSplit}`,
            display: 'flex',
            justifyContent: 'flex-end',
            alignItems: 'center',
            paddingInline: 24,
          }}
        >
          <Button
            type={selected === CONNECTION_SETTINGS_PATH ? 'primary' : 'text'}
            icon={<ApiOutlined />}
            onClick={() => navigate(`/${CONNECTION_SETTINGS_PATH}`)}
          >
            連線設定
          </Button>
        </Header>
        <Content style={{ padding: 24, overflow: 'auto' }}>
          <Outlet />
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
      </Layout>
    </Layout>
  );
}

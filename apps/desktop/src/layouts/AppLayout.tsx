import {
  ApiOutlined,
  DownOutlined,
  KeyOutlined,
  LogoutOutlined,
  UserOutlined,
} from '@ant-design/icons';
import { App, Button, Dropdown, Layout, Menu, Modal, Space, Typography, theme } from 'antd';
import { useState } from 'react';
import { Outlet, useLocation, useNavigate } from 'react-router';

import { errorMessage } from '../api/client';
import { useCurrentUser, usePermissions } from '../auth/permission';
import { useLogout } from '../auth/session';
import { ChangePasswordForm } from '../components/ChangePasswordForm';
import { StatusBar } from '../components/StatusBar';
import { CONNECTION_SETTINGS_PATH, MODULES } from '../modules';

const { Header, Sider, Content, Footer } = Layout;

/** 主畫面框架：左側模組選單、上方標題列（連線設定、使用者選單）、中間內容、底部狀態列。 */
export function AppLayout() {
  const navigate = useNavigate();
  const location = useLocation();
  const { token } = theme.useToken();
  const { message } = App.useApp();
  const user = useCurrentUser();
  const can = usePermissions();
  const logout = useLogout();
  const [changePasswordOpen, setChangePasswordOpen] = useState(false);

  // 選單依權限顯示；頁面本身也有檢查（router.tsx 的 RequirePermission）
  const modules = MODULES.filter((m) => !m.permission || can(m.permission));

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
          items={modules.map((m) => ({ key: m.path, icon: m.icon, label: m.label }))}
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
          <Space>
            <Button
              type={selected === CONNECTION_SETTINGS_PATH ? 'primary' : 'text'}
              icon={<ApiOutlined />}
              onClick={() => navigate(`/${CONNECTION_SETTINGS_PATH}`)}
            >
              連線設定
            </Button>
            <Dropdown
              trigger={['click']}
              menu={{
                items: [
                  { key: 'change-password', icon: <KeyOutlined />, label: '修改密碼' },
                  { type: 'divider' },
                  { key: 'logout', icon: <LogoutOutlined />, label: '登出' },
                ],
                onClick: ({ key }) => {
                  if (key === 'change-password') {
                    setChangePasswordOpen(true);
                  } else if (key === 'logout') {
                    logout.mutate(undefined, {
                      onError: (err) => message.error(errorMessage(err)),
                    });
                  }
                },
              }}
            >
              <Button type="text" icon={<UserOutlined />} loading={logout.isPending}>
                <Space size={4}>
                  {user?.display_name}
                  <DownOutlined />
                </Space>
              </Button>
            </Dropdown>
          </Space>
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
      <Modal
        title="修改密碼"
        open={changePasswordOpen}
        onCancel={() => setChangePasswordOpen(false)}
        footer={null}
        destroyOnHidden
      >
        <ChangePasswordForm
          onSuccess={() => {
            setChangePasswordOpen(false);
            message.success('密碼已修改');
          }}
        />
      </Modal>
    </Layout>
  );
}

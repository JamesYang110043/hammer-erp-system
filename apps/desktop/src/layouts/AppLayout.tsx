import {
  ApiOutlined,
  DownOutlined,
  KeyOutlined,
  LogoutOutlined,
  UserOutlined,
} from '@ant-design/icons';
import { App, Button, Dropdown, Layout, Menu, Modal, Space, Typography, theme } from 'antd';
import type { MenuProps } from 'antd';
import { useEffect, useState } from 'react';
import { Outlet, useLocation, useNavigate } from 'react-router';

import { errorMessage } from '../api/client';
import { useEntities } from '../api/schema';
import { useCurrentUser, usePermissions } from '../auth/permission';
import { useLogout } from '../auth/session';
import { ChangePasswordForm } from '../components/ChangePasswordForm';
import { StatusBar } from '../components/StatusBar';
import { PERMISSIONS } from '../bindings';
import type { EntityDefinition } from '../bindings';
import {
  CONNECTION_SETTINGS_PATH,
  ENTITIES_PATH,
  ENTITY_MENU_AFTER,
  MASTER_DATA_MODULES,
  MODULES,
  masterDataEntities,
  menuEntityGroups,
} from '../modules';
import { INVENTORY_MODULES } from '../inventoryModules';
import { entityIcon } from '../pages/schema/entityIcons';
import { INVENTORY_GROUP_LABEL, inventoryEntities } from '../records/inventory';
import { INVOICING_GROUP_LABEL, invoicingEntities } from '../records/invoicing';
import { PURCHASING_GROUP_LABEL, purchasingEntities } from '../records/purchasing';
import { SALES_GROUP_LABEL, salesEntities } from '../records/sales';
import { MASTER_DATA_LABEL } from '../schema/entities';
import { loadOpenKeys, saveOpenKeys, sectionOf, withSectionOpen } from './navSections';
import type { NavSection } from './navSections';

const { Header, Sider, Content, Footer } = Layout;

type MenuItem = NonNullable<MenuProps['items']>[number];

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

  // 資料類別選單需要 `record.read`；新增或修改資料類別後（invalidate `entities`）自動更新
  const canReadRecords = can(PERMISSIONS.RECORD_READ);
  const entities = useEntities({ enabled: canReadRecords });
  const entityItem = (e: EntityDefinition): MenuItem => ({
    key: `${ENTITIES_PATH}/${e.key}`,
    icon: entityIcon(e.icon),
    label: e.label,
  });
  // 可收折的區塊（子選單）；沒有任何項目（例如沒有權限）的區塊不顯示
  const sections: NavSection[] = [];
  const section = (key: string, label: string, children: MenuItem[]): MenuItem | null => {
    if (children.length === 0) {
      return null;
    }
    sections.push({ key, itemKeys: children.map((c) => String(c?.key)) });
    return { key, label, children };
  };
  const divider = (key: string): MenuItem => ({ type: 'divider', key });
  /** 區塊後面接一條分隔線；區塊不顯示時兩者都不顯示。 */
  const sectionWithDivider = (key: string, label: string, children: MenuItem[]): MenuItem[] => {
    const item = section(key, label, children);
    return item ? [item, divider(`${key}-divider`)] : [];
  };

  // 系統主檔（商品、客戶、廠商…）與幣別、匯率放在選單最上方
  const masterDataItems = sectionWithDivider(`${ENTITIES_PATH}-system`, MASTER_DATA_LABEL, [
    ...(canReadRecords ? masterDataEntities(entities.data ?? []) : []).map(entityItem),
    ...MASTER_DATA_MODULES.filter((m) => !m.permission || can(m.permission)).map((m): MenuItem => ({
      key: m.path,
      icon: m.icon,
      label: m.label,
    })),
  ]);

  // 進貨：採購單、進貨單（docs/architecture.md §8.8）
  const purchasingItems = sectionWithDivider(
    'purchasing-group',
    PURCHASING_GROUP_LABEL,
    (canReadRecords ? purchasingEntities(entities.data ?? []) : []).map(entityItem),
  );

  // 銷貨：訂單、出貨單、銷貨退回（docs/architecture.md §8.9）
  const salesItems = sectionWithDivider(
    'sales-group',
    SALES_GROUP_LABEL,
    (canReadRecords ? salesEntities(entities.data ?? []) : []).map(entityItem),
  );

  // 發票：銷項發票、銷貨折讓、進項憑證、進貨折讓（docs/architecture.md §8.11）
  const invoicingItems = sectionWithDivider(
    'invoicing-group',
    INVOICING_GROUP_LABEL,
    (canReadRecords ? invoicingEntities(entities.data ?? []) : []).map(entityItem),
  );

  // 庫存：盤點調整單與庫存查詢（docs/architecture.md §8.10）
  const inventoryItems = sectionWithDivider('inventory-group', INVENTORY_GROUP_LABEL, [
    ...(canReadRecords ? inventoryEntities(entities.data ?? []) : []).map(entityItem),
    ...INVENTORY_MODULES.filter((m) => !m.permission || can(m.permission)).map((m): MenuItem => ({
      key: m.path,
      icon: m.icon,
      label: m.label,
    })),
  ]);

  // 自訂資料類別依種類分組（主檔、單據），組內依選單排序
  const entityItems: MenuItem[] = canReadRecords
    ? menuEntityGroups(entities.data ?? []).flatMap((g) => {
        const item = section(`${ENTITIES_PATH}-${g.kind}`, g.label, g.entities.map(entityItem));
        return item ? [item] : [];
      })
    : [];

  // 選單依權限顯示；頁面本身也有檢查（router.tsx 的 RequirePermission）
  const menuItems: MenuItem[] = [
    ...masterDataItems,
    ...purchasingItems,
    ...salesItems,
    ...invoicingItems,
    ...inventoryItems,
    ...MODULES.filter((m) => !m.permission || can(m.permission)).flatMap((m) => {
      const item: MenuItem = { key: m.path, icon: m.icon, label: m.label };
      return m.path === ENTITY_MENU_AFTER ? [item, ...entityItems] : [item];
    }),
  ];

  // `/customers/123` → `customers`；`/entities/repair` → `entities/repair`
  const [first = '', second = ''] = location.pathname.split('/').slice(1);
  const selected = first === ENTITIES_PATH ? `${first}/${second}` : first;

  // 區塊展開狀態：沿用上次的狀態，並展開目前頁面所屬的區塊。
  // 切換頁面、或資料類別載入後才知道目前頁面所屬的區塊時 → 自動展開該區塊
  // （使用者在同一頁收起區塊則維持收起）
  const currentSection = sectionOf(sections, selected);
  const [openKeys, setOpenKeys] = useState<readonly string[]>(() =>
    withSectionOpen(loadOpenKeys(), currentSection),
  );
  const pageSection = `${selected}\n${currentSection ?? ''}`;
  const [openedFor, setOpenedFor] = useState(pageSection);
  if (openedFor !== pageSection) {
    setOpenedFor(pageSection);
    setOpenKeys(withSectionOpen(openKeys, currentSection));
  }
  useEffect(() => saveOpenKeys(openKeys), [openKeys]);

  return (
    <Layout style={{ height: '100vh' }}>
      <Sider width={200} theme="light" style={{ borderRight: `1px solid ${token.colorSplit}` }}>
        {/* 標題固定在上方，只有選單捲動；overscroll-behavior 讓捲到底時不帶動右側內容 */}
        <div style={{ display: 'flex', flexDirection: 'column', height: '100%' }}>
          <div style={{ padding: '16px 24px', flexShrink: 0 }}>
            <Typography.Title level={4} style={{ margin: 0 }}>
              Hammer ERP
            </Typography.Title>
          </div>
          <nav
            aria-label="主選單"
            data-testid="app-nav"
            style={{ flex: 1, minHeight: 0, overflowY: 'auto', overscrollBehavior: 'contain' }}
          >
            <Menu
              mode="inline"
              selectedKeys={[selected]}
              openKeys={[...openKeys]}
              onOpenChange={setOpenKeys}
              items={menuItems}
              onClick={({ key }) => navigate(`/${key}`)}
              style={{ borderInlineEnd: 'none' }}
            />
          </nav>
        </div>
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
        <Content
          data-testid="app-content"
          style={{ padding: 24, overflow: 'auto', overscrollBehavior: 'contain' }}
        >
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

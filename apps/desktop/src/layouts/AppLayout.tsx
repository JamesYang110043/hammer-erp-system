import {
  ApiOutlined,
  AppstoreOutlined,
  ContainerOutlined,
  DownOutlined,
  FileTextOutlined,
  FolderOutlined,
  KeyOutlined,
  LogoutOutlined,
  MenuFoldOutlined,
  MenuUnfoldOutlined,
  ProfileOutlined,
  ShopOutlined,
  ShoppingCartOutlined,
} from '@ant-design/icons';
import {
  App,
  Avatar,
  Breadcrumb,
  Button,
  Dropdown,
  Layout,
  Menu,
  Modal,
  Space,
  Tooltip,
  theme,
} from 'antd';
import type { BreadcrumbProps, MenuProps } from 'antd';
import { useEffect, useState } from 'react';
import type { CSSProperties, ReactNode } from 'react';
import { Link, Outlet, useLocation, useNavigate } from 'react-router';

import { errorMessage } from '../api/client';
import { useEntities } from '../api/schema';
import { useCurrentUser, usePermissions } from '../auth/permission';
import { useLogout } from '../auth/session';
import { BrandLogo } from '../components/BrandLogo';
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
import { INVENTORY_GROUP_LABEL, STOCK_PATH, inventoryEntities } from '../records/inventory';
import { INVOICING_GROUP_LABEL, invoicingEntities } from '../records/invoicing';
import { PURCHASING_GROUP_LABEL, purchasingEntities } from '../records/purchasing';
import { SALES_GROUP_LABEL, salesEntities } from '../records/sales';
import { MASTER_DATA_LABEL } from '../schema/entities';
import { BRAND_COLORS } from '../theme';
import {
  loadCollapsed,
  loadOpenKeys,
  navTrail,
  saveCollapsed,
  saveOpenKeys,
  sectionOf,
  withSectionOpen,
} from './navSections';
import type { NavNode, NavSection } from './navSections';

const { Header, Sider, Content, Footer } = Layout;

type MenuItem = NonNullable<MenuProps['items']>[number];

/** 側邊欄寬度（展開／收合）。 */
const SIDER_WIDTH = 216;
const SIDER_COLLAPSED_WIDTH = 64;

/** 選單項目轉成麵包屑用的結構（只取文字標籤的項目）。 */
function toNavNodes(items: readonly MenuItem[]): NavNode[] {
  return items.flatMap((item): NavNode[] => {
    if (!item || !('label' in item) || typeof item.label !== 'string') {
      return [];
    }
    const children = 'children' in item && item.children ? toNavNodes(item.children) : undefined;
    return [{ key: String(item.key), label: item.label, children }];
  });
}

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
  // 側邊欄收合狀態記在本機，重新開啟程式後維持
  const [collapsed, setCollapsed] = useState(loadCollapsed);
  useEffect(() => saveCollapsed(collapsed), [collapsed]);

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
  // 區塊標題平常只顯示文字（較小字級、次要色）；側邊欄收合時改顯示圖示
  const section = (
    key: string,
    label: string,
    icon: ReactNode,
    children: MenuItem[],
  ): MenuItem | null => {
    if (children.length === 0) {
      return null;
    }
    sections.push({ key, itemKeys: children.map((c) => String(c?.key)) });
    return { key, label, children, icon: collapsed ? icon : undefined, className: 'nav-section' };
  };
  const divider = (key: string): MenuItem => ({
    type: 'divider',
    key,
    style: { borderColor: BRAND_COLORS.siderDivider },
  });
  /** 區塊後面接一條分隔線；區塊不顯示時兩者都不顯示。 */
  const sectionWithDivider = (
    key: string,
    label: string,
    icon: ReactNode,
    children: MenuItem[],
  ): MenuItem[] => {
    const item = section(key, label, icon, children);
    return item ? [item, divider(`${key}-divider`)] : [];
  };

  // 系統主檔（商品、客戶、廠商…）與幣別、匯率放在選單最上方
  const masterDataItems = sectionWithDivider(
    `${ENTITIES_PATH}-system`,
    MASTER_DATA_LABEL,
    <AppstoreOutlined />,
    [
      ...(canReadRecords ? masterDataEntities(entities.data ?? []) : []).map(entityItem),
      ...MASTER_DATA_MODULES.filter((m) => !m.permission || can(m.permission)).map(
        (m): MenuItem => ({
          key: m.path,
          icon: m.icon,
          label: m.label,
        }),
      ),
    ],
  );

  // 進貨：採購單、進貨單（docs/architecture.md §8.8）
  const purchasingItems = sectionWithDivider(
    'purchasing-group',
    PURCHASING_GROUP_LABEL,
    <ShoppingCartOutlined />,
    (canReadRecords ? purchasingEntities(entities.data ?? []) : []).map(entityItem),
  );

  // 銷貨：訂單、出貨單、銷貨退回（docs/architecture.md §8.9）
  const salesItems = sectionWithDivider(
    'sales-group',
    SALES_GROUP_LABEL,
    <ShopOutlined />,
    (canReadRecords ? salesEntities(entities.data ?? []) : []).map(entityItem),
  );

  // 發票：銷項發票、銷貨折讓、進項憑證、進貨折讓（docs/architecture.md §8.11）
  const invoicingItems = sectionWithDivider(
    'invoicing-group',
    INVOICING_GROUP_LABEL,
    <FileTextOutlined />,
    (canReadRecords ? invoicingEntities(entities.data ?? []) : []).map(entityItem),
  );

  // 庫存：盤點調整單與庫存查詢（docs/architecture.md §8.10）
  const inventoryItems = sectionWithDivider(
    'inventory-group',
    INVENTORY_GROUP_LABEL,
    <ContainerOutlined />,
    [
      ...(canReadRecords ? inventoryEntities(entities.data ?? []) : []).map(entityItem),
      ...INVENTORY_MODULES.filter((m) => !m.permission || can(m.permission)).map((m): MenuItem => ({
        key: m.path,
        icon: m.icon,
        label: m.label,
      })),
    ],
  );

  // 自訂資料類別依種類分組（主檔、單據），組內依選單排序
  const entityItems: MenuItem[] = canReadRecords
    ? menuEntityGroups(entities.data ?? []).flatMap((g) => {
        const item = section(
          `${ENTITIES_PATH}-${g.kind}`,
          g.label,
          g.kind === 'master' ? <FolderOutlined /> : <ProfileOutlined />,
          g.entities.map(entityItem),
        );
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

  // 麵包屑：區塊 › 頁面（› 下一層，例如資料類別的欄位設定、單據明細）
  const pageTrail = navTrail(toNavNodes(menuItems), selected);
  const third = location.pathname.split('/')[3];
  const subPage = (() => {
    if (!second || (first === ENTITIES_PATH && !third)) {
      return [];
    }
    if (first === ENTITIES_PATH) {
      return [third === 'new' ? '新增' : '明細'];
    }
    if (first === 'schema') {
      const label = entities.data?.find((e) => e.key === second)?.label ?? second;
      return third === 'form-layout' ? [label, '表單版面'] : [label];
    }
    if (first === STOCK_PATH) {
      return ['商品庫存'];
    }
    return [];
  })();
  const trail =
    pageTrail.length === 0 && first === CONNECTION_SETTINGS_PATH ? ['連線設定'] : pageTrail;
  const breadcrumbItems: BreadcrumbProps['items'] = [
    ...trail.map((title, i) => ({
      key: `page-${i}`,
      // 有下一層時，頁面名稱可以點回列表
      title:
        i === trail.length - 1 && subPage.length > 0 ? (
          <Link to={`/${selected}`}>{title}</Link>
        ) : (
          title
        ),
    })),
    ...subPage.map((title, i) => ({ key: `sub-${i}`, title })),
  ];

  const navStyle = {
    flex: 1,
    minHeight: 0,
    overflowY: 'auto',
    overscrollBehavior: 'contain',
    '--nav-section-color': BRAND_COLORS.siderMutedText,
    '--nav-section-hover-color': BRAND_COLORS.siderText,
  } as CSSProperties;

  const userName = user?.display_name ?? '';

  return (
    <Layout style={{ height: '100vh' }}>
      <Sider
        width={SIDER_WIDTH}
        collapsedWidth={SIDER_COLLAPSED_WIDTH}
        collapsible
        collapsed={collapsed}
        trigger={null}
        theme="dark"
      >
        {/* Logo 固定在上方，只有選單捲動；overscroll-behavior 讓捲到底時不帶動右側內容 */}
        <div style={{ display: 'flex', flexDirection: 'column', height: '100%' }}>
          <div
            data-testid="app-logo"
            style={{
              height: 64,
              flexShrink: 0,
              display: 'flex',
              alignItems: 'center',
              justifyContent: collapsed ? 'center' : 'flex-start',
              paddingInline: collapsed ? 0 : 20,
              borderBottom: `1px solid ${BRAND_COLORS.siderDivider}`,
            }}
          >
            <BrandLogo color={BRAND_COLORS.siderText} collapsed={collapsed} />
          </div>
          <nav aria-label="主選單" data-testid="app-nav" className="app-nav" style={navStyle}>
            <Menu
              theme="dark"
              mode="inline"
              selectedKeys={[selected]}
              // 收合時子選單改為浮出，展開狀態交給 antd（不影響展開時記住的區塊）
              {...(collapsed ? {} : { openKeys: [...openKeys], onOpenChange: setOpenKeys })}
              items={menuItems}
              onClick={({ key }) => navigate(`/${key}`)}
              style={{ borderInlineEnd: 'none', paddingBlock: 8 }}
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
            justifyContent: 'space-between',
            alignItems: 'center',
            gap: 16,
            paddingInline: '12px 24px',
            height: 56,
            lineHeight: '56px',
          }}
        >
          <Space size={8} style={{ minWidth: 0 }}>
            <Tooltip title={collapsed ? '展開側邊欄' : '收合側邊欄'}>
              <Button
                type="text"
                aria-label={collapsed ? '展開側邊欄' : '收合側邊欄'}
                icon={collapsed ? <MenuUnfoldOutlined /> : <MenuFoldOutlined />}
                onClick={() => setCollapsed((c) => !c)}
              />
            </Tooltip>
            <nav aria-label="目前位置" data-testid="app-breadcrumb">
              <Breadcrumb items={breadcrumbItems} />
            </nav>
          </Space>
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
              <Button type="text" loading={logout.isPending} style={{ paddingInline: 8 }}>
                <Space size={8}>
                  <Avatar
                    size={28}
                    aria-hidden
                    style={{ background: token.colorPrimary, fontSize: token.fontSize }}
                  >
                    {userName.slice(0, 1)}
                  </Avatar>
                  {userName}
                  <DownOutlined style={{ fontSize: 10, color: token.colorTextTertiary }} />
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

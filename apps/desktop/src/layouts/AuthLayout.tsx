import { ApiOutlined } from '@ant-design/icons';
import { Button, Card, Layout, Modal, Typography, theme } from 'antd';
import { useState } from 'react';
import type { ReactNode } from 'react';

import { BrandLogo, PRODUCT_NAME } from '../components/BrandLogo';
import { StatusBar } from '../components/StatusBar';
import { ConnectionSettingsPage } from '../pages/ConnectionSettingsPage';
import { BRAND_COLORS } from '../theme';

const { Content, Footer } = Layout;

/** 品牌區塊下方的簡短標語。 */
export const AUTH_TAGLINE = '進銷存與庫存管理系統';

/**
 * 登入前的畫面框架（建立管理者、登入、強制改密碼、連線失敗）。
 *
 * 寬視窗左右分欄：左側品牌色區塊（Logo、系統名稱、標語），右側表單卡片；
 * 視窗變窄時（styles.css 的 `.auth-brand`）只顯示表單卡片，Logo 移到卡片上方。
 * 右上角可以開啟連線設定：伺服器網址錯誤時，不用登入也能修改。
 */
export function AuthLayout({ title, children }: { title: string; children: ReactNode }) {
  const { token } = theme.useToken();
  const [settingsOpen, setSettingsOpen] = useState(false);

  return (
    <Layout style={{ height: '100vh' }}>
      <div style={{ flex: 1, minHeight: 0, display: 'flex' }}>
        <aside
          className="auth-brand"
          data-testid="auth-brand"
          style={{
            background: `linear-gradient(160deg, ${BRAND_COLORS.siderBg} 0%, ${BRAND_COLORS.primary} 100%)`,
            color: BRAND_COLORS.siderText,
          }}
        >
          <BrandLogo collapsed />
          <div>
            <Typography.Title
              level={1}
              style={{
                color: BRAND_COLORS.siderText,
                margin: 0,
                fontSize: 36,
                letterSpacing: '0.01em',
              }}
            >
              {PRODUCT_NAME}
            </Typography.Title>
            <Typography.Paragraph
              style={{ color: BRAND_COLORS.siderText, fontSize: 16, margin: '12px 0 0' }}
            >
              {AUTH_TAGLINE}
            </Typography.Paragraph>
          </div>
        </aside>
        <Content
          style={{
            position: 'relative',
            flex: 1,
            minWidth: 0,
            display: 'flex',
            flexDirection: 'column',
            alignItems: 'center',
            padding: '72px 24px 32px',
            overflow: 'auto',
          }}
        >
          <Button
            type="text"
            icon={<ApiOutlined />}
            onClick={() => setSettingsOpen(true)}
            style={{ position: 'absolute', top: 16, right: 16 }}
          >
            連線設定
          </Button>
          <div style={{ margin: 'auto 0', width: 400, maxWidth: '100%' }}>
            <div className="auth-compact-logo" style={{ marginBottom: 24 }}>
              <BrandLogo />
            </div>
            <Card
              title={title}
              style={{ boxShadow: token.boxShadowSecondary }}
              styles={{
                header: { fontSize: token.fontSizeHeading4, minHeight: 64, borderBottom: 'none' },
                body: { paddingTop: 8 },
              }}
            >
              {children}
            </Card>
          </div>
        </Content>
      </div>
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

import { ArrowLeftOutlined } from '@ant-design/icons';
import { Button, Card, Space, Typography } from 'antd';
import type { CardProps } from 'antd';
import type { ReactNode } from 'react';

export interface PageHeaderProps {
  /** 頁面標題 */
  title: ReactNode;
  /** 標題前的圖示（資料類別圖示等） */
  icon?: ReactNode;
  /** 標題後的標籤（單據狀態、未存檔…） */
  tags?: ReactNode;
  /** 標題下方的說明文字 */
  description?: ReactNode;
  /** 右側動作列（重新整理、新增、返回…） */
  actions?: ReactNode;
  /** 標題左側的返回按鈕；`backLabel` 為按鈕的無障礙名稱 */
  onBack?: () => void;
  backLabel?: string;
}

/**
 * 頁面標題列（docs/architecture.md「視覺設計」）：左側標題、標籤與說明，右側動作列。
 *
 * 各頁一律用這個元件（或 {@link Page}）呈現標題，標題位置、字級、動作列位置一致；
 * 頁面內的區塊標題用 `SectionTitle` 或 Card 的 `title`。
 */
export function PageHeader({
  title,
  icon,
  tags,
  description,
  actions,
  onBack,
  backLabel,
}: PageHeaderProps) {
  return (
    <header className="page-header" data-testid="page-header">
      <div className="page-header-main">
        <div className="page-header-heading">
          {onBack && (
            <Button
              type="text"
              icon={<ArrowLeftOutlined />}
              aria-label={backLabel ?? '返回'}
              onClick={onBack}
            />
          )}
          <Typography.Title level={4} className="page-header-title">
            {icon && <span className="page-header-icon">{icon}</span>}
            {title}
          </Typography.Title>
          {tags && <Space size={4}>{tags}</Space>}
        </div>
        {description && (
          <Typography.Text type="secondary" className="page-header-description">
            {description}
          </Typography.Text>
        )}
      </div>
      {actions && (
        <Space wrap className="page-header-actions">
          {actions}
        </Space>
      )}
    </header>
  );
}

/**
 * 一般頁面：{@link PageHeader} 加上一張放內容的卡片。
 *
 * 有多張卡片的頁面（例如商品庫存）直接用 `PageHeader`，下方自行排列卡片。
 */
export function Page({
  children,
  loading,
  cardProps,
  ...header
}: PageHeaderProps & {
  children?: ReactNode;
  /** 載入中：卡片顯示骨架 */
  loading?: boolean;
  cardProps?: Omit<CardProps, 'title' | 'extra' | 'loading'>;
}) {
  return (
    <div className="page">
      <PageHeader {...header} />
      <Card loading={loading} {...cardProps}>
        {children}
      </Card>
    </div>
  );
}

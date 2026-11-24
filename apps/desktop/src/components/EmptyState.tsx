import {
  ClockCircleOutlined,
  InboxOutlined,
  LockOutlined,
  SearchOutlined,
  StopOutlined,
} from '@ant-design/icons';
import { Typography, theme } from 'antd';
import type { ReactNode } from 'react';

/** 空狀態的種類：圖示與預設文字。 */
const EMPTY_STATES = {
  empty: { icon: <InboxOutlined />, title: '沒有資料' },
  forbidden: { icon: <LockOutlined />, title: '沒有權限' },
  notFound: { icon: <SearchOutlined />, title: '找不到資料' },
  disabled: { icon: <StopOutlined />, title: '已停用' },
  comingSoon: { icon: <ClockCircleOutlined />, title: '此功能尚未開放' },
} as const;

export type EmptyStateKind = keyof typeof EMPTY_STATES;

/**
 * 一致的空狀態（無資料、無權限、找不到、停用、尚未開放）：圖示＋標題＋引導文字。
 *
 * 表格等元件的「沒有資料」也透過 ConfigProvider 的 `renderEmpty` 使用（`compact`）。
 */
export function EmptyState({
  kind = 'empty',
  title,
  description,
  action,
  compact = false,
}: {
  kind?: EmptyStateKind;
  /** 預設為該種類的文字 */
  title?: ReactNode;
  /** 引導文字（下一步可以做什麼） */
  description?: ReactNode;
  action?: ReactNode;
  /** 表格、下拉選單內使用的精簡版（較小的圖示與間距） */
  compact?: boolean;
}) {
  const { token } = theme.useToken();
  const preset = EMPTY_STATES[kind];
  return (
    <div
      role="status"
      data-testid="empty-state"
      style={{
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        gap: compact ? 4 : 8,
        padding: compact ? `${token.paddingSM}px 0` : `${token.paddingXL}px 0`,
        textAlign: 'center',
      }}
    >
      <span
        aria-hidden
        style={{ fontSize: compact ? 28 : 40, color: token.colorTextQuaternary, lineHeight: 1 }}
      >
        {preset.icon}
      </span>
      <Typography.Text
        type={compact ? 'secondary' : undefined}
        strong={!compact}
        style={{ fontSize: compact ? token.fontSize : token.fontSizeLG }}
      >
        {title ?? preset.title}
      </Typography.Text>
      {description && <Typography.Text type="secondary">{description}</Typography.Text>}
      {action && <div style={{ marginTop: 8 }}>{action}</div>}
    </div>
  );
}

import { Typography, theme } from 'antd';
import type { CSSProperties, ReactNode } from 'react';

/**
 * 頁面內的區塊標題（表單區塊、明細表、群組）：左側主色色條（`colorPrimary`，版面見 styles.css `.section-title`）。
 *
 * `extra` 放在標題右側（工具列、勾選框等），與標題垂直置中對齊。
 */
export function SectionTitle({
  children,
  extra,
  style,
}: {
  children: ReactNode;
  extra?: ReactNode;
  style?: CSSProperties;
}) {
  const { token } = theme.useToken();
  const title = (
    <Typography.Title
      level={5}
      className="section-title"
      style={{ borderInlineStartColor: token.colorPrimary }}
    >
      {children}
    </Typography.Title>
  );
  if (!extra) {
    return <div style={{ marginBottom: 12, ...style }}>{title}</div>;
  }
  return (
    <div className="section-title-row" style={style}>
      {title}
      {extra}
    </div>
  );
}

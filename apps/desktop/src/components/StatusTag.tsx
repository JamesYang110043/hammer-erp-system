import { Tag } from 'antd';
import type { CSSProperties, ReactNode } from 'react';

import { optionColors, toneColors } from '../status';
import type { StatusTone } from '../status';
import type { TagColors } from '../theme';

function tagStyle(colors: TagColors | undefined, style?: CSSProperties): CSSProperties | undefined {
  return colors
    ? { color: colors.text, background: colors.bg, borderColor: colors.border, ...style }
    : style;
}

/** 狀態標籤：顏色依語意狀態（src/status.ts），不直接指定色名。 */
export function StatusTag({
  tone,
  icon,
  style,
  children,
}: {
  tone: StatusTone;
  icon?: ReactNode;
  style?: CSSProperties;
  children: ReactNode;
}) {
  return (
    <Tag icon={icon} style={tagStyle(toneColors(tone), style)} data-tone={tone}>
      {children}
    </Tag>
  );
}

/** select 選項標籤：顏色依選項的色名（對應到色票）；沒有設定顏色時為預設樣式。 */
export function OptionColorTag({
  color,
  children,
}: {
  color: string | null | undefined;
  children: ReactNode;
}) {
  return (
    <Tag style={tagStyle(optionColors(color))} data-color={color ?? undefined}>
      {children}
    </Tag>
  );
}

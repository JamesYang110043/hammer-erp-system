import { ColorPicker, Select, Space, Typography, theme } from 'antd';
import { useState } from 'react';

import { THEME_PRESETS, appPalette, normalizeHex, saveStoredPreset } from '../theme';
import type { ThemePresetKey } from '../theme';

const PRESET_LABELS: Record<ThemePresetKey, string> = { navy: '深藍', teal: '藍綠' };

/**
 * 主色方案比較用的開發工具（#229）：只在 `pnpm dev`／`pnpm tauri dev` 顯示。
 * 切換後存在本機並重新載入，讓 antd 以新的主色重新推導整組 token。
 * 固定在左下角（側邊欄底部的空白處），不影響版面。
 */
export function DevThemeSwitcher() {
  const { token } = theme.useToken();
  const [mode, setMode] = useState<string>(
    () =>
      (Object.keys(THEME_PRESETS) as ThemePresetKey[]).find(
        (k) => THEME_PRESETS[k] === appPalette,
      ) ?? 'custom',
  );
  const [custom, setCustom] = useState(appPalette.primary);

  const apply = (value: string) => {
    saveStoredPreset(value);
    // 網址上的 ?theme= 會蓋過本機的選擇，切換時一併移除
    const { pathname, hash } = window.location;
    window.history.replaceState(null, '', pathname + hash);
    window.location.reload();
  };

  return (
    <Space
      size="small"
      style={{
        position: 'fixed',
        left: 8,
        bottom: 40,
        zIndex: 1000,
        padding: '4px 8px',
        background: token.colorBgElevated,
        borderRadius: token.borderRadius,
        boxShadow: token.boxShadowSecondary,
      }}
    >
      <Typography.Text type="secondary">主色（開發用）</Typography.Text>
      <Select
        size="small"
        aria-label="主色方案"
        value={mode}
        style={{ width: 96 }}
        options={[
          ...(Object.keys(THEME_PRESETS) as ThemePresetKey[]).map((k) => ({
            value: k,
            label: PRESET_LABELS[k],
          })),
          { value: 'custom', label: '自訂' },
        ]}
        onChange={(v) => {
          setMode(v);
          // 自訂：先顯示色彩選擇器，選好顏色才套用
          if (v !== 'custom') {
            apply(v);
          }
        }}
      />
      {mode === 'custom' && (
        <ColorPicker
          size="small"
          disabledAlpha
          value={custom}
          onChangeComplete={(c) => {
            const hex = normalizeHex(c.toHexString());
            if (hex) {
              setCustom(hex);
              apply(hex.slice(1));
            }
          }}
        />
      )}
    </Space>
  );
}

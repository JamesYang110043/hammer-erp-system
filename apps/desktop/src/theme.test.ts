import { describe, expect, it } from 'vitest';

import { BRAND_COLORS, CONTROL_HEIGHT, TAG_PALETTE, appTheme } from './theme';

/** WCAG 相對亮度對比（docs/architecture.md「視覺設計」：文字對比 ≥ 4.5:1）。 */
function contrast(a: string, b: string) {
  const lum = (hex: string) => {
    const [r, g, bl] = [1, 3, 5].map((i) => {
      const c = Number.parseInt(hex.slice(i, i + 2), 16) / 255;
      return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
    });
    return 0.2126 * r! + 0.7152 * g! + 0.0722 * bl!;
  };
  const [hi, lo] = [lum(a), lum(b)].sort((x, y) => y - x);
  return (hi! + 0.05) / (lo! + 0.05);
}

describe('appTheme', () => {
  it('keeps the control height and applies the navy brand color', () => {
    expect(appTheme.token?.controlHeight).toBe(CONTROL_HEIGHT);
    expect(appTheme.token?.colorPrimary).toBe('#1F4E8C');
    expect(appTheme.components?.Menu?.darkItemSelectedBg).toBe(BRAND_COLORS.primary);
    expect(appTheme.components?.Layout?.siderBg).toBe(BRAND_COLORS.siderBg);
  });

  it('sidebar text has enough contrast on the sidebar background', () => {
    for (const bg of [BRAND_COLORS.siderBg, BRAND_COLORS.siderSubBg]) {
      expect(contrast(BRAND_COLORS.siderText, bg)).toBeGreaterThanOrEqual(4.5);
      expect(contrast(BRAND_COLORS.siderItemText, bg)).toBeGreaterThanOrEqual(4.5);
      expect(contrast(BRAND_COLORS.siderMutedText, bg)).toBeGreaterThanOrEqual(4.5);
    }
    // 選中項目：白字在主色上
    expect(contrast('#FFFFFF', BRAND_COLORS.primary)).toBeGreaterThanOrEqual(4.5);
  });

  it('tag_palette_text_has_enough_contrast', () => {
    for (const [name, c] of Object.entries(TAG_PALETTE)) {
      expect(contrast(c.text, c.bg), name).toBeGreaterThanOrEqual(4.5);
      // 白底的表格中也要清楚
      expect(contrast(c.text, '#FFFFFF'), name).toBeGreaterThanOrEqual(4.5);
    }
  });
});

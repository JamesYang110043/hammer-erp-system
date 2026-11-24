import { describe, expect, it } from 'vitest';

import {
  DEFAULT_PRESET,
  THEME_PRESETS,
  buildTheme,
  customPalette,
  mixHex,
  normalizeHex,
  resolvePalette,
} from './theme';

describe('normalizeHex', () => {
  it('accepts 6-digit hex with or without #', () => {
    expect(normalizeHex('1f4e8c')).toBe('#1F4E8C');
    expect(normalizeHex(' #0f8b8d ')).toBe('#0F8B8D');
  });

  it('rejects anything else', () => {
    expect(normalizeHex('navy')).toBeNull();
    expect(normalizeHex('#fff')).toBeNull();
    expect(normalizeHex('#1F4E8CFF')).toBeNull();
  });
});

describe('mixHex', () => {
  it('mixes toward the target color', () => {
    expect(mixHex('#FFFFFF', '#000000', 0)).toBe('#FFFFFF');
    expect(mixHex('#FFFFFF', '#000000', 1)).toBe('#000000');
    expect(mixHex('#FF0000', '#0000FF', 0.5)).toBe('#800080');
  });
});

describe('resolvePalette', () => {
  const navy = THEME_PRESETS.navy;

  it('always uses the default preset outside dev mode', () => {
    expect(resolvePalette(false, '?theme=teal', 'teal')).toBe(THEME_PRESETS[DEFAULT_PRESET]);
  });

  it('prefers the URL over the stored choice', () => {
    expect(resolvePalette(true, '?theme=teal', 'navy')).toBe(THEME_PRESETS.teal);
    expect(resolvePalette(true, '', 'teal')).toBe(THEME_PRESETS.teal);
  });

  it('derives a palette from a custom hex color', () => {
    expect(resolvePalette(true, '?theme=8e3b8a', null)).toEqual(customPalette('#8E3B8A'));
  });

  it('falls back to the default for unknown values', () => {
    expect(resolvePalette(true, '?theme=pink', null)).toBe(navy);
    expect(resolvePalette(true, '', null)).toBe(navy);
  });
});

describe('buildTheme', () => {
  it('keeps the control height and applies the brand color', () => {
    const theme = buildTheme(THEME_PRESETS.teal);
    expect(theme.token?.controlHeight).toBe(36);
    expect(theme.token?.colorPrimary).toBe('#0F8B8D');
    expect(theme.components?.Menu?.darkItemSelectedBg).toBe('#0F8B8D');
    expect(theme.components?.Layout?.siderBg).toBe(THEME_PRESETS.teal.siderBg);
  });
});

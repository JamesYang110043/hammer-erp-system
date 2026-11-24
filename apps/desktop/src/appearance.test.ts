import { describe, expect, it } from 'vitest';

import {
  APPEARANCE_STORAGE_KEY,
  loadAppearance,
  resolveColorMode,
  saveAppearance,
} from './appearance';

describe('外觀偏好', () => {
  it('round_trips_through_local_storage', () => {
    saveAppearance('dark');
    expect(window.localStorage.getItem(APPEARANCE_STORAGE_KEY)).toBe('dark');
    expect(loadAppearance()).toBe('dark');
    saveAppearance('light');
    expect(loadAppearance()).toBe('light');
  });

  it('defaults_to_system_when_missing_or_invalid', () => {
    window.localStorage.removeItem(APPEARANCE_STORAGE_KEY);
    expect(loadAppearance()).toBe('system');
    window.localStorage.setItem(APPEARANCE_STORAGE_KEY, 'sepia');
    expect(loadAppearance()).toBe('system');
    window.localStorage.removeItem(APPEARANCE_STORAGE_KEY);
  });

  it('system_follows_prefers_color_scheme', () => {
    expect(resolveColorMode('system', true)).toBe('dark');
    expect(resolveColorMode('system', false)).toBe('light');
    expect(resolveColorMode('light', true)).toBe('light');
    expect(resolveColorMode('dark', false)).toBe('dark');
  });
});

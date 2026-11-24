import { describe, expect, it } from 'vitest';

import {
  NAV_COLLAPSED_STORAGE_KEY,
  NAV_OPEN_KEYS_STORAGE_KEY,
  loadCollapsed,
  loadOpenKeys,
  navTrail,
  saveCollapsed,
  saveOpenKeys,
  sectionOf,
  withSectionOpen,
} from './navSections';

const SECTIONS = [
  { key: 'master', itemKeys: ['entities/customer', 'exchange-rates'] },
  { key: 'inventory', itemKeys: ['stock'] },
];

describe('sectionOf', () => {
  it('finds_the_section_of_the_selected_item', () => {
    expect(sectionOf(SECTIONS, 'entities/customer')).toBe('master');
    expect(sectionOf(SECTIONS, 'stock')).toBe('inventory');
  });

  it('returns_undefined_outside_sections', () => {
    expect(sectionOf(SECTIONS, 'users')).toBeUndefined();
    expect(sectionOf([], 'stock')).toBeUndefined();
  });
});

describe('withSectionOpen', () => {
  it('adds_the_section', () => {
    expect(withSectionOpen(['inventory'], 'master')).toEqual(['inventory', 'master']);
  });

  it('keeps_the_same_list_when_already_open_or_no_section', () => {
    const keys = ['master'];
    expect(withSectionOpen(keys, 'master')).toBe(keys);
    expect(withSectionOpen(keys, undefined)).toBe(keys);
  });
});

describe('loadOpenKeys / saveOpenKeys', () => {
  it('round_trips', () => {
    saveOpenKeys(['master', 'inventory']);
    expect(loadOpenKeys()).toEqual(['master', 'inventory']);
  });

  it('empty_when_nothing_saved', () => {
    expect(loadOpenKeys()).toEqual([]);
  });

  it.each([['not json'], ['{"a":1}'], ['"master"']])('ignores_malformed_value (%s)', (raw) => {
    window.localStorage.setItem(NAV_OPEN_KEYS_STORAGE_KEY, raw);
    expect(loadOpenKeys()).toEqual([]);
  });

  it('drops_non_string_entries', () => {
    window.localStorage.setItem(NAV_OPEN_KEYS_STORAGE_KEY, '["master", 1, null]');
    expect(loadOpenKeys()).toEqual(['master']);
  });
});

describe('loadCollapsed / saveCollapsed', () => {
  it('round_trips', () => {
    saveCollapsed(true);
    expect(window.localStorage.getItem(NAV_COLLAPSED_STORAGE_KEY)).toBe('true');
    expect(loadCollapsed()).toBe(true);
    saveCollapsed(false);
    expect(loadCollapsed()).toBe(false);
  });

  it('expanded_when_nothing_saved', () => {
    window.localStorage.removeItem(NAV_COLLAPSED_STORAGE_KEY);
    expect(loadCollapsed()).toBe(false);
  });
});

describe('navTrail', () => {
  const NODES = [
    {
      key: 'master',
      label: '基本資料',
      children: [
        { key: 'entities/customer', label: '客戶' },
        { key: 'exchange-rates', label: '匯率' },
      ],
    },
    { key: 'users', label: '帳號管理' },
  ];

  it('returns_section_and_item', () => {
    expect(navTrail(NODES, 'entities/customer')).toEqual(['基本資料', '客戶']);
  });

  it('returns_top_level_item_alone', () => {
    expect(navTrail(NODES, 'users')).toEqual(['帳號管理']);
  });

  it('is_empty_outside_the_menu', () => {
    expect(navTrail(NODES, 'connection')).toEqual([]);
  });
});

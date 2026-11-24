import { describe, expect, it } from 'vitest';

import { DOCUMENT_STATUSES } from './records/documents';
import {
  DOCUMENT_STATUS_TONES,
  OPTION_COLOR_NAMES,
  TONE_BADGE_STATUS,
  TONE_TAG_COLOR,
  optionColors,
  toneColors,
} from './status';
import { TAG_PALETTE } from './theme';

describe('語意狀態', () => {
  it('document_statuses_map_to_tones', () => {
    expect(DOCUMENT_STATUS_TONES.draft).toBe('neutral');
    expect(DOCUMENT_STATUS_TONES.posted).toBe('success');
    expect(DOCUMENT_STATUS_TONES.closed).toBe('success');
    expect(DOCUMENT_STATUS_TONES.confirmed).toBe('info');
    expect(DOCUMENT_STATUS_TONES.partially_received).toBe('warning');
    expect(DOCUMENT_STATUS_TONES.partially_shipped).toBe('warning');
    expect(DOCUMENT_STATUS_TONES.void).toBe('danger');
    // 每一種單據狀態都有對應
    for (const status of DOCUMENT_STATUSES) {
      expect(DOCUMENT_STATUS_TONES[status]).toBeDefined();
    }
  });

  it('tones_use_the_tag_palette_and_badge_status', () => {
    expect(toneColors('success')).toBe(TAG_PALETTE.green);
    expect(toneColors('danger')).toBe(TAG_PALETTE.red);
    expect(toneColors('neutral')).toBe(TAG_PALETTE.gray);
    expect(TONE_TAG_COLOR.warning).toBe('gold');
    expect(TONE_BADGE_STATUS).toEqual({
      neutral: 'default',
      success: 'success',
      warning: 'warning',
      danger: 'error',
      info: 'processing',
    });
  });
});

describe('optionColors', () => {
  it('palette_names_map_directly', () => {
    for (const { value } of OPTION_COLOR_NAMES) {
      expect(optionColors(value)).toBe(TAG_PALETTE[value]);
    }
  });

  it('legacy_antd_names_map_to_the_nearest_color', () => {
    expect(optionColors('volcano')).toBe(TAG_PALETTE.orange);
    expect(optionColors('geekblue')).toBe(TAG_PALETTE.blue);
    expect(optionColors('Success')).toBe(TAG_PALETTE.green);
  });

  it('unknown_or_empty_is_default', () => {
    expect(optionColors(null)).toBeUndefined();
    expect(optionColors('')).toBeUndefined();
    expect(optionColors('#123456')).toBeUndefined();
  });
});

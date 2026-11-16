import { describe, expect, it } from 'vitest';

import { formatFileSize, progressPercent } from './attachments';

describe('formatFileSize', () => {
  it('uses_binary_units', () => {
    expect(formatFileSize(0)).toBe('0 B');
    expect(formatFileSize(1023)).toBe('1023 B');
    expect(formatFileSize(1024)).toBe('1 KB');
    expect(formatFileSize(1536)).toBe('1.5 KB');
    expect(formatFileSize(20 * 1024 * 1024)).toBe('20 MB');
    expect(formatFileSize(150 * 1024 * 1024)).toBe('150 MB');
    expect(formatFileSize(3 * 1024 ** 4)).toBe('3072 GB');
    expect(formatFileSize(null)).toBe('');
  });
});

describe('progressPercent', () => {
  it('rounds_down_and_handles_empty_file', () => {
    const p = (sent: number | null, total: number | null) =>
      progressPercent({ file_name: 'a', sent_bytes: sent, total_bytes: total });
    expect(p(0, 2048)).toBe(0);
    expect(p(1023, 2048)).toBe(49);
    expect(p(2048, 2048)).toBe(100);
    expect(p(0, 0)).toBe(100);
    expect(p(null, null)).toBe(100);
  });
});

import dayjs from 'dayjs';
import type { Dayjs } from 'dayjs';
import utc from 'dayjs/plugin/utc';

dayjs.extend(utc);

/** Asia/Taipei 固定 UTC+8，無日光節約（docs/architecture.md §5、D13）。 */
export const TAIPEI_OFFSET_MINUTES = 8 * 60;

/** 伺服器的 RFC 3339 時間 → 台北時間字串，例如 `2026-11-13 09:05:30`。 */
export function formatTaipei(iso: string, format = 'YYYY-MM-DD HH:mm:ss'): string {
  return dayjs(iso).utcOffset(TAIPEI_OFFSET_MINUTES).format(format);
}

/**
 * 日期（使用者選的「哪一天」）→ 台北時間當天 00:00 的 RFC 3339 字串。
 *
 * 只取年月日，不受電腦本身時區影響：`2026-11-01` → `2026-11-01T00:00:00+08:00`。
 */
export function taipeiStartOfDay(date: Dayjs): string {
  return `${date.format('YYYY-MM-DD')}T00:00:00+08:00`;
}

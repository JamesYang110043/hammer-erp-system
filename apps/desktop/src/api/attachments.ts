import { Channel } from '@tauri-apps/api/core';

import { commands } from '../bindings';
import type { Attachment_Serialize as Attachment, UploadProgress } from '../bindings';
import { unwrap } from './client';

export type { Attachment, UploadProgress };

/**
 * 開啟選擇檔案的對話框並上傳（需要 `record.write`，伺服器檢查）。使用者取消時回傳 `null`。
 *
 * 選檔與讀檔都在 Rust 端進行，前端拿不到本機路徑（docs/architecture.md §3.3）。
 * 選好檔案後先回報一次進度（已送出 0），之後每送出一段回報一次。
 * 上傳後附件尚未綁定，要把 id 寫入 `file` 欄位並存檔；超過大小上限時伺服器回傳 `413`。
 */
export function uploadAttachment(
  onProgress: (progress: UploadProgress) => void,
): Promise<Attachment | null> {
  return unwrap(commands.uploadAttachment(new Channel<UploadProgress>(onProgress)));
}

/**
 * 開啟另存新檔的對話框（預設檔名 `fileName`），下載附件存到選擇的位置（需要 `record.read`）。
 *
 * 存檔成功回傳實際存檔的檔名（使用者可能在對話框改名；不含路徑）；使用者取消時回傳 `null`。
 */
export function downloadAttachment(id: string, fileName: string): Promise<string | null> {
  return unwrap(commands.downloadAttachment(id, fileName));
}

/** 檔案大小的顯示文字：`1536` → `1.5 KB`。 */
export function formatFileSize(bytes: number | null | undefined): string {
  if (bytes === null || bytes === undefined) {
    return '';
  }
  const units = ['KB', 'MB', 'GB'];
  if (bytes < 1024) {
    return `${bytes} B`;
  }
  let size = bytes / 1024;
  let unit = 0;
  while (size >= 1024 && unit < units.length - 1) {
    size /= 1024;
    unit += 1;
  }
  const text = size >= 100 ? size.toFixed(0) : size.toFixed(1).replace(/\.0$/, '');
  return `${text} ${units[unit]}`;
}

/** 上傳進度的百分比（0–100 的整數）。 */
export function progressPercent(progress: UploadProgress): number {
  const total = progress.total_bytes ?? 0;
  if (total <= 0) {
    return 100;
  }
  return Math.min(100, Math.floor(((progress.sent_bytes ?? 0) / total) * 100));
}

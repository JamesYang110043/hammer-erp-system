/**
 * 左側選單可收折區塊（基本資料、進貨、銷貨…）的展開狀態。
 *
 * 展開狀態存在 localStorage，重新開啟程式後維持；目前頁面所屬的區塊一律展開。
 */

/** localStorage 的 key。 */
export const NAV_OPEN_KEYS_STORAGE_KEY = 'hammer.nav.openKeys';

/** 選單中的一個區塊：區塊 key 與它底下項目的 key。 */
export interface NavSection {
  key: string;
  itemKeys: readonly string[];
}

/** `selectedKey` 所屬區塊的 key；不在任何區塊內（例如帳號管理）時為 `undefined`。 */
export function sectionOf(
  sections: readonly NavSection[],
  selectedKey: string,
): string | undefined {
  return sections.find((s) => s.itemKeys.includes(selectedKey))?.key;
}

/** 把 `sectionKey` 加入展開清單；已經展開或沒有區塊時回傳原本的清單。 */
export function withSectionOpen(openKeys: readonly string[], sectionKey: string | undefined) {
  return sectionKey === undefined || openKeys.includes(sectionKey)
    ? openKeys
    : [...openKeys, sectionKey];
}

/** 上次儲存的展開區塊；沒有存過、格式錯誤或無法存取 localStorage 時為空清單。 */
export function loadOpenKeys(): string[] {
  try {
    const raw = window.localStorage.getItem(NAV_OPEN_KEYS_STORAGE_KEY);
    const parsed: unknown = raw === null ? [] : JSON.parse(raw);
    return Array.isArray(parsed) ? parsed.filter((k): k is string => typeof k === 'string') : [];
  } catch {
    return [];
  }
}

/** 儲存展開區塊；無法寫入時忽略（只影響下次開啟的預設狀態）。 */
export function saveOpenKeys(openKeys: readonly string[]) {
  try {
    window.localStorage.setItem(NAV_OPEN_KEYS_STORAGE_KEY, JSON.stringify(openKeys));
  } catch {
    // 忽略
  }
}

/** 側邊欄收合狀態的 localStorage key。 */
export const NAV_COLLAPSED_STORAGE_KEY = 'hammer.nav.collapsed';

/** 上次的側邊欄收合狀態；沒有存過或無法存取 localStorage 時為展開（`false`）。 */
export function loadCollapsed(): boolean {
  try {
    return window.localStorage.getItem(NAV_COLLAPSED_STORAGE_KEY) === 'true';
  } catch {
    return false;
  }
}

/** 儲存側邊欄收合狀態；無法寫入時忽略。 */
export function saveCollapsed(collapsed: boolean) {
  try {
    window.localStorage.setItem(NAV_COLLAPSED_STORAGE_KEY, String(collapsed));
  } catch {
    // 忽略
  }
}

/** 選單項目（只取麵包屑需要的部分）。 */
export interface NavNode {
  key: string;
  label: string;
  children?: readonly NavNode[];
}

/**
 * `selectedKey` 在選單中的位置：區塊內的項目為 `[區塊, 項目]`，最上層的項目為 `[項目]`；
 * 不在選單中時為空清單。
 */
export function navTrail(nodes: readonly NavNode[], selectedKey: string): string[] {
  for (const node of nodes) {
    if (node.key === selectedKey) {
      return [node.label];
    }
    const child = node.children?.find((c) => c.key === selectedKey);
    if (child) {
      return [node.label, child.label];
    }
  }
  return [];
}

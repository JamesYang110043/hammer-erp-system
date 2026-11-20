/**
 * 表單版面設計器的狀態與操作（docs/architecture.md §7.7；畫面在 `pages/schema/FormLayoutDesignerPage`）。
 *
 * 狀態是版面 config 加上每個項目的拖曳 id（欄位 `field:<代碼>`、系統區塊 `block:<代碼>`、
 * 空白格 `spacer:<流水號>`）。所有操作都是純函式（{@link designerReducer}），有單元測試。
 *
 * 規則：
 * - 系統區塊不能刪除，只能移動；刪除區塊時，其中的系統區塊移到第一個剩下的區塊最後
 * - 刪除欄位或刪除區塊後，欄位回到「尚未放進版面」（存檔後伺服器附加在最後的「其他欄位」）
 * - 至少保留一個區塊
 * - 欄數減少時，超過的 `span` 調整為欄數
 */
import type { FormItem, FormLayoutConfig, FormSection } from './formLayout';
import { MAX_FORM_COLUMNS } from './formLayout';

export type DesignItem = FormItem & { id: string };

export interface DesignSection extends Omit<FormSection, 'items'> {
  items: DesignItem[];
}

export interface DesignerState {
  sections: DesignSection[];
  /** 下一個空白格的流水號。 */
  nextSpacer: number;
}

/** 區塊的拖曳 id。 */
export const sectionId = (key: string) => `section:${key}`;
/** 區塊內放置區（空的區塊也能放入項目）的 id。 */
export const dropId = (key: string) => `drop:${key}`;

/** 拖曳 id 對應的區塊代碼（區塊本身或其放置區）；不是區塊時回傳 `undefined`。 */
export function sectionKeyOfId(id: string): string | undefined {
  for (const prefix of ['section:', 'drop:']) {
    if (id.startsWith(prefix)) {
      return id.slice(prefix.length);
    }
  }
  return undefined;
}

function itemId(item: FormItem, spacer: () => number): string {
  switch (item.type) {
    case 'field':
      return `field:${item.field}`;
    case 'block':
      return `block:${item.block}`;
    case 'spacer':
      return `spacer:${spacer()}`;
  }
}

export function toDesign(config: FormLayoutConfig): DesignerState {
  let next = 0;
  const spacer = () => next++;
  const sections = config.sections.map((s) => ({
    ...s,
    items: s.items.map((item) => ({ ...item, id: itemId(item, spacer) })),
  }));
  return { sections, nextSpacer: next };
}

function withoutId(item: DesignItem): FormItem {
  const copy: Partial<DesignItem> = { ...item };
  delete copy.id;
  return copy as FormItem;
}

export function toConfig(state: DesignerState): FormLayoutConfig {
  return {
    sections: state.sections.map((s) => ({
      ...s,
      items: s.items.map(withoutId),
    })),
  };
}

export type DesignerAction =
  | { type: 'reset'; config: FormLayoutConfig }
  | { type: 'addSection' }
  | { type: 'removeSection'; key: string }
  | {
      type: 'updateSection';
      key: string;
      patch: Partial<Pick<FormSection, 'title' | 'columns' | 'collapsible' | 'collapsed'>>;
    }
  | { type: 'moveSection'; key: string; to: number }
  /** 把項目移到 `section` 的第 `index` 個位置（同一個區塊內為重新排序）。 */
  | { type: 'moveItem'; id: string; section: string; index: number }
  | { type: 'setSpan'; id: string; span: number }
  | { type: 'addSpacer'; section: string }
  /** 從「尚未放進版面」加入欄位或系統區塊，放在區塊最後。 */
  | { type: 'addItem'; section: string; item: FormItem }
  /** 移除欄位或空白格（系統區塊不能移除）。 */
  | { type: 'removeItem'; id: string };

/** 新區塊的代碼：`section_1`、`section_2`…中第一個沒有使用的。 */
function newSectionKey(sections: readonly DesignSection[]): string {
  const used = new Set(sections.map((s) => s.key));
  let n = 1;
  while (used.has(`section_${n}`)) {
    n += 1;
  }
  return `section_${n}`;
}

function clampSpan(item: DesignItem, columns: number): DesignItem {
  return item.type === 'block'
    ? item
    : { ...item, span: Math.min(Math.max(item.span, 1), columns) };
}

/** 項目所在的區塊位置與在區塊中的位置；找不到時回傳 `undefined`。 */
export function findItem(
  state: DesignerState,
  id: string,
): { section: number; index: number; item: DesignItem } | undefined {
  for (const [section, s] of state.sections.entries()) {
    const index = s.items.findIndex((i) => i.id === id);
    const item = s.items[index];
    if (item) {
      return { section, index, item };
    }
  }
  return undefined;
}

export function designerReducer(state: DesignerState, action: DesignerAction): DesignerState {
  const mapSection = (key: string, f: (s: DesignSection) => DesignSection) => ({
    ...state,
    sections: state.sections.map((s) => (s.key === key ? f(s) : s)),
  });

  switch (action.type) {
    case 'reset':
      return toDesign(action.config);

    case 'addSection':
      return {
        ...state,
        sections: [
          ...state.sections,
          {
            key: newSectionKey(state.sections),
            title: '新區塊',
            columns: 2,
            collapsible: false,
            collapsed: false,
            items: [],
          },
        ],
      };

    case 'removeSection': {
      const removed = state.sections.find((s) => s.key === action.key);
      const rest = state.sections.filter((s) => s.key !== action.key);
      const [first, ...others] = rest;
      if (!removed || !first) {
        return state;
      }
      const blocks = removed.items.filter((i) => i.type === 'block');
      return { ...state, sections: [{ ...first, items: [...first.items, ...blocks] }, ...others] };
    }

    case 'updateSection':
      return mapSection(action.key, (s) => {
        const next = { ...s, ...action.patch };
        next.columns = Math.min(Math.max(next.columns, 1), MAX_FORM_COLUMNS);
        if (!next.collapsible) {
          next.collapsed = false;
        }
        return { ...next, items: next.items.map((i) => clampSpan(i, next.columns)) };
      });

    case 'moveSection': {
      const from = state.sections.findIndex((s) => s.key === action.key);
      const moving = state.sections[from];
      if (!moving) {
        return state;
      }
      const sections = state.sections.filter((_, i) => i !== from);
      const to = Math.min(Math.max(action.to, 0), sections.length);
      sections.splice(to, 0, moving);
      return { ...state, sections };
    }

    case 'moveItem': {
      const found = findItem(state, action.id);
      const target = state.sections.findIndex((s) => s.key === action.section);
      if (!found || target < 0) {
        return state;
      }
      const sections = state.sections.map((s) => ({ ...s, items: [...s.items] }));
      sections[found.section]!.items.splice(found.index, 1);
      const dest = sections[target]!;
      const index = Math.min(Math.max(action.index, 0), dest.items.length);
      dest.items.splice(index, 0, clampSpan(found.item, dest.columns));
      return { ...state, sections };
    }

    case 'setSpan': {
      const found = findItem(state, action.id);
      const section = found && state.sections[found.section];
      if (!found || !section || found.item.type === 'block') {
        return state;
      }
      return mapSection(section.key, (s) => ({
        ...s,
        items: s.items.map((i) =>
          i.id === action.id ? clampSpan({ ...i, span: action.span } as DesignItem, s.columns) : i,
        ),
      }));
    }

    case 'addSpacer':
      return {
        ...mapSection(action.section, (s) => ({
          ...s,
          items: [...s.items, { type: 'spacer', span: 1, id: `spacer:${state.nextSpacer}` }],
        })),
        nextSpacer: state.nextSpacer + 1,
      };

    case 'addItem': {
      const id = itemId(action.item, () => state.nextSpacer);
      if (findItem(state, id)) {
        return state;
      }
      return mapSection(action.section, (s) => ({
        ...s,
        items: [...s.items, clampSpan({ ...action.item, id }, s.columns)],
      }));
    }

    case 'removeItem': {
      const found = findItem(state, action.id);
      const section = found && state.sections[found.section];
      if (!found || !section || found.item.type === 'block') {
        return state;
      }
      return mapSection(section.key, (s) => ({
        ...s,
        items: s.items.filter((i) => i.id !== action.id),
      }));
    }
  }
}

/** 版面中已經放入的欄位代碼與系統區塊代碼。 */
export function placedKeys(state: DesignerState): { fields: Set<string>; blocks: Set<string> } {
  const fields = new Set<string>();
  const blocks = new Set<string>();
  for (const s of state.sections) {
    for (const item of s.items) {
      if (item.type === 'field') {
        fields.add(item.field);
      } else if (item.type === 'block') {
        blocks.add(item.block);
      }
    }
  }
  return { fields, blocks };
}

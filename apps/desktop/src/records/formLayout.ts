/**
 * 表單版面（docs/architecture.md §7.7）的前端規則。
 *
 * 伺服器回傳的版面已經正規化（略過停用的欄位、附加未放入的欄位與系統區塊）；這裡再依
 * 「畫面上實際能顯示的欄位與明細表」整理一次，確保任何欄位、系統區塊都不會因為版面而消失：
 *
 * - 讀取版面失敗時，依欄位定義產生與伺服器相同的預設版面（{@link generatedFormLayout}）
 * - 版面中不能顯示的欄位（`is_visible = false`、表單不支援的型態）與不認得的系統區塊略過
 * - 能顯示、但不在版面中的欄位與系統區塊，依伺服器相同的規則附加（{@link resolveFormLayout}）
 */
import type {
  FormItem_Serialize as FormItem,
  FormLayoutConfig_Serialize as FormLayoutConfig,
  FormSection_Serialize as FormSection,
} from '../bindings';
import { PURCHASE_ALLOWANCE, PURCHASE_INVOICE, SALES_ALLOWANCE, SALES_INVOICE } from './invoicing';
import { SALES_RETURN, SHIPMENT } from './sales';

export type { FormItem, FormLayoutConfig, FormSection };

/** 系統區塊代碼（`hammer_shared::form_layout`）。 */
export const FORM_BLOCKS = {
  recordNo: 'record_no',
  shipmentPicker: 'shipment_picker',
  receiptPicker: 'receipt_picker',
  allowanceAvailable: 'allowance_available',
  usedLots: 'used_lots',
} as const;

/** 明細表系統區塊代碼的前綴（`lines:<明細子表代碼>`）。 */
export const LINES_BLOCK_PREFIX = 'lines:';

/** 自動產生的版面唯一的區塊代碼。 */
export const MAIN_SECTION_KEY = 'main';
/** 自動附加未放入版面的欄位的區塊代碼與標題。 */
export const OTHER_SECTION_KEY = '_other';
export const OTHER_SECTION_TITLE = '其他欄位';
/** 自動附加未放入版面的系統區塊的區塊代碼。 */
export const BLOCKS_SECTION_KEY = '_blocks';

/** 一個區塊最多的欄數。 */
export const MAX_FORM_COLUMNS = 4;

export function linesBlock(detailKey: string): string {
  return `${LINES_BLOCK_PREFIX}${detailKey}`;
}

/** 明細表系統區塊的明細子表代碼；不是明細表時回傳 `undefined`。 */
export function linesBlockDetail(code: string): string | undefined {
  return code.startsWith(LINES_BLOCK_PREFIX) ? code.slice(LINES_BLOCK_PREFIX.length) : undefined;
}

/** 系統區塊沒有放進版面時自動附加的位置：欄位之前或之後。 */
export type BlockPlacement = 'top' | 'bottom';

/**
 * 資料類別適用的系統區塊，依自動產生時的順序（與 `hammer_shared::form_layout::system_blocks` 相同）：
 * 可折讓金額 → 單號 →（欄位）→ 選擇出貨單／進貨單 → 各明細表 → 已用批號表。
 */
export function systemBlocks(
  entityKey: string,
  detailKeys: readonly string[],
): { code: string; placement: BlockPlacement }[] {
  const blocks: { code: string; placement: BlockPlacement }[] = [];
  if (entityKey === SALES_ALLOWANCE || entityKey === PURCHASE_ALLOWANCE) {
    blocks.push({ code: FORM_BLOCKS.allowanceAvailable, placement: 'top' });
  }
  blocks.push({ code: FORM_BLOCKS.recordNo, placement: 'top' });
  if (entityKey === SALES_INVOICE) {
    blocks.push({ code: FORM_BLOCKS.shipmentPicker, placement: 'bottom' });
  }
  if (entityKey === PURCHASE_INVOICE) {
    blocks.push({ code: FORM_BLOCKS.receiptPicker, placement: 'bottom' });
  }
  for (const key of detailKeys) {
    blocks.push({ code: linesBlock(key), placement: 'bottom' });
  }
  if (entityKey === SHIPMENT || entityKey === SALES_RETURN) {
    blocks.push({ code: FORM_BLOCKS.usedLots, placement: 'bottom' });
  }
  return blocks;
}

/** 表單上能顯示的內容：資料類別代碼、欄位代碼（依顯示順序）與明細子表代碼。 */
export interface FormContent {
  entityKey: string;
  fieldKeys: readonly string[];
  detailKeys: readonly string[];
}

function plainSection(key: string, title: string, items: FormItem[]): FormSection {
  return { key, title, columns: 1, collapsible: false, collapsed: false, items };
}

const fieldItem = (field: string): FormItem => ({ type: 'field', field, span: 1 });
const blockItem = (block: string): FormItem => ({ type: 'block', block });

/** 分組預設版面每個區塊的欄數（`hammer_shared::form_layout::PRESET_COLUMNS`）。 */
export const PRESET_COLUMNS = 2;

interface PresetSection {
  key: string;
  title: string;
  /** `[欄位代碼, span]`，依顯示順序。 */
  fields: readonly (readonly [string, number])[];
}

/**
 * 沒有設定版面時使用的分組預設版面（與 `hammer_shared::form_layout::preset_sections` 相同，
 * 共用測試資料 `crates/shared/testdata/generated_form_layouts.json`）：客戶、廠商。
 */
export function presetSections(entityKey: string): PresetSection[] | undefined {
  if (entityKey !== 'customer' && entityKey !== 'supplier') {
    return undefined;
  }
  const supplier = entityKey === 'supplier';
  const one = (...keys: string[]) => keys.map((k) => [k, 1] as const);
  return [
    {
      key: 'basic',
      title: '基本資料',
      fields: [
        ...one('code', 'name', 'short_name', 'tax_id', 'phone', 'fax', 'email', 'address'),
        ...(supplier ? one('country') : []),
        ...one('is_active'),
      ],
    },
    { key: 'invoice', title: '發票資訊', fields: one('invoice_title', 'invoice_address') },
    {
      key: 'terms',
      title: '交易條件',
      fields: [
        ...one('payment_terms_days', 'credit_limit'),
        ...(supplier ? one('default_currency_code', 'bank_info') : []),
      ],
    },
    { key: 'other', title: '其他', fields: [['notes', PRESET_COLUMNS]] },
  ];
}

/**
 * 沒有設定版面（或讀取失敗）時的版面（與伺服器的 `generated_config` 相同）：
 *
 * - 客戶、廠商（{@link presetSections}）：分組、2 欄的區塊，略過不能顯示的欄位與沒有欄位的
 *   區塊，再依 {@link resolveFormLayout} 附加單號、未列出的欄位（含自訂欄位）與明細表
 * - 其他資料類別：一個單欄、沒有標題的區塊，依序為欄位之前的系統區塊、欄位、欄位之後的系統區塊
 */
export function generatedFormLayout(content: FormContent): FormLayoutConfig {
  const preset = presetSections(content.entityKey);
  if (preset) {
    const shown = new Set(content.fieldKeys);
    const sections = preset.flatMap((p): FormSection[] => {
      const items = p.fields
        .filter(([key]) => shown.has(key))
        .map(([field, span]): FormItem => ({ type: 'field', field, span }));
      return items.length > 0
        ? [
            {
              key: p.key,
              title: p.title,
              columns: PRESET_COLUMNS,
              collapsible: false,
              collapsed: false,
              items,
            },
          ]
        : [];
    });
    return resolveFormLayout({ sections }, content);
  }
  const blocks = systemBlocks(content.entityKey, content.detailKeys);
  const items = [
    ...blocks.filter((b) => b.placement === 'top').map((b) => blockItem(b.code)),
    ...content.fieldKeys.map(fieldItem),
    ...blocks.filter((b) => b.placement === 'bottom').map((b) => blockItem(b.code)),
  ];
  return { sections: [plainSection(MAIN_SECTION_KEY, '', items)] };
}

/**
 * 依畫面上能顯示的內容整理版面（見檔案開頭的說明）：
 *
 * - 略過不能顯示的欄位、不適用的系統區塊與重複的項目；欄數限制在 1–4，`span` 不超過欄數
 * - 能顯示但不在版面中的欄位附加到「其他欄位」區塊
 * - 不在版面中的系統區塊：欄位之前的插入第一個區塊的最前面，其他附加到最後
 */
export function resolveFormLayout(
  config: FormLayoutConfig,
  content: FormContent,
): FormLayoutConfig {
  const shownFields = new Set(content.fieldKeys);
  const blocks = systemBlocks(content.entityKey, content.detailKeys);
  const blockCodes = new Set(blocks.map((b) => b.code));
  const seenFields = new Set<string>();
  const seenBlocks = new Set<string>();

  const sections: FormSection[] = config.sections.map((section) => {
    const columns = Math.min(Math.max(section.columns, 1), MAX_FORM_COLUMNS);
    const clamp = (span: number) => Math.min(Math.max(span, 1), columns);
    const items = section.items.flatMap((item): FormItem[] => {
      switch (item.type) {
        case 'field':
          if (!shownFields.has(item.field) || seenFields.has(item.field)) {
            return [];
          }
          seenFields.add(item.field);
          return [{ ...item, span: clamp(item.span) }];
        case 'spacer':
          return [{ ...item, span: clamp(item.span) }];
        case 'block':
          if (!blockCodes.has(item.block) || seenBlocks.has(item.block)) {
            return [];
          }
          seenBlocks.add(item.block);
          return [item];
      }
    });
    return { ...section, columns, collapsed: section.collapsed && section.collapsible, items };
  });

  const top = blocks.filter((b) => b.placement === 'top' && !seenBlocks.has(b.code));
  if (top.length > 0) {
    const first = sections[0] ?? plainSection(MAIN_SECTION_KEY, '', []);
    sections[0] = { ...first, items: [...top.map((b) => blockItem(b.code)), ...first.items] };
  }
  const missingFields = content.fieldKeys.filter((key) => !seenFields.has(key)).map(fieldItem);
  if (missingFields.length > 0) {
    const index = sections.findIndex((s) => s.key === OTHER_SECTION_KEY);
    const other = sections[index];
    if (other) {
      sections[index] = { ...other, items: [...other.items, ...missingFields] };
    } else {
      sections.push(plainSection(OTHER_SECTION_KEY, OTHER_SECTION_TITLE, missingFields));
    }
  }
  const bottom = blocks.filter((b) => b.placement === 'bottom' && !seenBlocks.has(b.code));
  if (bottom.length > 0) {
    sections.push(
      plainSection(
        BLOCKS_SECTION_KEY,
        '',
        bottom.map((b) => blockItem(b.code)),
      ),
    );
  }
  return { sections };
}

/** 區塊中的欄位代碼（收折的區塊有驗證錯誤時，用來找出要展開的區塊）。 */
export function sectionFieldKeys(section: FormSection): string[] {
  return section.items.flatMap((item) => (item.type === 'field' ? [item.field] : []));
}

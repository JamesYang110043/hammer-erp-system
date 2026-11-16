import type { EntityDefinition, FieldDefinition } from '../bindings';
import { isFormulaOperand, isNumericField } from './fieldSettings';

/** 插入欄位清單的一個選項：顯示欄位名稱，插入的是欄位代碼（§7.6）。 */
export interface FormulaRefOption {
  /** 顯示的名稱。 */
  label: string;
  /** 插入公式的文字，例如 `{qty}`、`{customer.rate}`、`SUM(lines.amount)`。 */
  value: string;
}

export interface FormulaRefGroup {
  label: string;
  options: FormulaRefOption[];
}

/** 可以引用的欄位：啟用中、型態可用於公式（{@link isFormulaOperand}）。 */
function usable(fields: readonly FieldDefinition[]): FieldDefinition[] {
  return fields.filter((f) => f.is_active && isFormulaOperand(f));
}

/**
 * 公式可以插入的欄位，依來源分組（docs/architecture.md §7.6）：
 *
 * - 本筆欄位 `{field_key}`：不含正在編輯的欄位（引用自己是循環）
 * - 每個啟用中的關聯欄位：目標資料類別的欄位 `{relation.field_key}`（最多一層）
 * - 每個明細子表（只有主表的公式可以使用）：數字欄位插入 `SUM(detail.field_key)`，
 *   其他欄位插入 `COUNT(detail.field_key)`；要改用 `AVG`、`MIN`、`MAX` 時直接修改函式名稱
 *
 * 只列出可以引用的欄位；引用是否合法仍以伺服器檢查為準。
 */
export function formulaRefGroups({
  fields,
  selfKey,
  relationTargets,
  details,
}: {
  /** 本筆（公式所在資料表）的欄位。 */
  fields: readonly FieldDefinition[];
  /** 正在編輯的欄位代碼；新增時為 `undefined`。 */
  selfKey?: string;
  /** 關聯欄位代碼 → 目標資料類別的欄位。沒有載入的關聯欄位不列出。 */
  relationTargets: ReadonlyMap<string, readonly FieldDefinition[]>;
  /** 有效的明細子表與其欄位；公式在明細子表時傳空陣列。 */
  details: readonly { entity: EntityDefinition; fields: readonly FieldDefinition[] }[];
}): FormulaRefGroup[] {
  const groups: FormulaRefGroup[] = [
    {
      label: '本筆欄位',
      options: usable(fields)
        .filter((f) => f.field_key !== selfKey)
        .map((f) => ({ label: f.label, value: `{${f.field_key}}` })),
    },
  ];
  for (const rel of fields) {
    const targets = relationTargets.get(rel.field_key);
    if (!rel.is_active || rel.data_type !== 'relation' || !targets) {
      continue;
    }
    groups.push({
      label: `關聯：${rel.label}`,
      options: usable(targets).map((f) => ({
        label: `${rel.label}．${f.label}`,
        value: `{${rel.field_key}.${f.field_key}}`,
      })),
    });
  }
  for (const detail of details) {
    groups.push({
      label: `明細：${detail.entity.label}`,
      options: usable(detail.fields).map((f) => {
        const numeric = isNumericField(f);
        return {
          label: `${f.label}（${numeric ? '合計' : '筆數'}）`,
          value: `${numeric ? 'SUM' : 'COUNT'}(${detail.entity.key}.${f.field_key})`,
        };
      }),
    });
  }
  return groups.filter((g) => g.options.length > 0);
}

/**
 * 在 `value` 的 `[start, end)`（UTF-16 位置，即 textarea 的 `selectionStart` / `selectionEnd`）
 * 換成 `text`，回傳新的值與插入後的游標位置。位置超出範圍時插在結尾。
 */
export function insertText(
  value: string,
  text: string,
  start: number | null | undefined,
  end: number | null | undefined,
): { value: string; cursor: number } {
  const from = Math.min(start ?? value.length, value.length);
  const to = Math.max(from, Math.min(end ?? from, value.length));
  return { value: value.slice(0, from) + text + value.slice(to), cursor: from + text.length };
}

/**
 * 依錯誤位置切開公式（位置是**字元**位移，與 `hammer_formula::Span` 相同，不是 UTF-16）。
 *
 * 範圍為空（`start === end`，例如公式不完整）時標示該位置的一個字元；位置在結尾時
 * `marked` 為空字串，由畫面補上標示。
 */
export function splitAtSpan(
  formula: string,
  start: number,
  end: number,
): { before: string; marked: string; after: string } {
  const chars = Array.from(formula);
  const from = Math.min(start, chars.length);
  const to = Math.min(Math.max(end, from + 1), chars.length);
  return {
    before: chars.slice(0, from).join(''),
    marked: chars.slice(from, to).join(''),
    after: chars.slice(to).join(''),
  };
}

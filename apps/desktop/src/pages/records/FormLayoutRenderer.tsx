import { Collapse, Typography } from 'antd';
import { useState } from 'react';
import type { CSSProperties, ReactNode } from 'react';

import { FORM_BLOCKS, sectionFieldKeys } from '../../records/formLayout';
import type { FormItem, FormLayoutConfig, FormSection } from '../../records/formLayout';

type CellStyle = CSSProperties & { '--span'?: number; '--columns'?: number };

/**
 * 依表單版面（docs/architecture.md §7.7）排列欄位與系統區塊。
 *
 * - 每個區塊是 `columns` 欄的格線（樣式在 styles.css），項目依 `span` 佔用欄數，空白格佔位；
 *   系統區塊（明細表等）佔滿整列，單號與欄位相同佔一格
 * - 項目依版面順序輸出，Tab 鍵的移動順序與畫面閱讀順序一致（左到右、上到下）
 * - 寬度不足時退回單欄（CSS container query）
 * - 可收折的區塊收折時內容仍保留在表單中（欄位的值與驗證不受影響）；`reveal` 中的欄位所在的
 *   區塊會自動展開（送出時有驗證錯誤）
 * - `renderField`、`renderBlock` 回傳 `null` 的項目不顯示（例如只在可以編輯時出現的區塊）；
 *   沒有任何可見項目的區塊整個不顯示
 */
export function FormLayoutRenderer({
  config,
  renderField,
  renderBlock,
  reveal,
}: {
  config: FormLayoutConfig;
  renderField: (fieldKey: string) => ReactNode;
  renderBlock: (code: string) => ReactNode;
  /** 有錯誤的欄位代碼；每次傳入新的陣列時，展開包含這些欄位的收折區塊。 */
  reveal?: readonly string[];
}) {
  const [open, setOpen] = useState<ReadonlySet<string>>(
    () => new Set(config.sections.filter((s) => s.collapsible && !s.collapsed).map((s) => s.key)),
  );

  // 收到新的錯誤欄位時展開所在的區塊（在 render 中依 prop 調整 state，不需要 effect）
  const [revealed, setRevealed] = useState(reveal);
  if (reveal !== revealed) {
    setRevealed(reveal);
    const keys = new Set(reveal ?? []);
    const targets = config.sections
      .filter((s) => s.collapsible && sectionFieldKeys(s).some((k) => keys.has(k)))
      .map((s) => s.key);
    if (targets.some((k) => !open.has(k))) {
      setOpen(new Set([...open, ...targets]));
    }
  }

  return (
    <>
      {config.sections.map((section) => {
        const cells = sectionCells(section, renderField, renderBlock);
        if (!cells.some((c) => !c.spacer)) {
          return null;
        }
        const grid = (
          <div className="form-layout-container">
            <div
              className={`form-layout-grid form-layout-cols-${section.columns}`}
              style={{ '--columns': section.columns } as CellStyle}
            >
              {cells.map((c) => c.node)}
            </div>
          </div>
        );
        const label = section.title || undefined;
        if (section.collapsible) {
          return (
            <section key={section.key} aria-label={label} data-section={section.key}>
              <Collapse
                size="small"
                style={{ marginBottom: 16 }}
                activeKey={open.has(section.key) ? [section.key] : []}
                onChange={(keys) => {
                  const active = (Array.isArray(keys) ? keys : [keys]).includes(section.key);
                  setOpen((prev) => {
                    const next = new Set(prev);
                    if (active) {
                      next.add(section.key);
                    } else {
                      next.delete(section.key);
                    }
                    return next;
                  });
                }}
                items={[
                  {
                    key: section.key,
                    label: section.title,
                    // 收折時也保留欄位（表單值與驗證）
                    forceRender: true,
                    children: grid,
                  },
                ]}
              />
            </section>
          );
        }
        return (
          <section key={section.key} aria-label={label} data-section={section.key}>
            {label && (
              <Typography.Title level={5} style={{ marginTop: 0 }}>
                {label}
              </Typography.Title>
            )}
            {grid}
          </section>
        );
      })}
    </>
  );
}

interface Cell {
  node: ReactNode;
  spacer: boolean;
}

function sectionCells(
  section: FormSection,
  renderField: (fieldKey: string) => ReactNode,
  renderBlock: (code: string) => ReactNode,
): Cell[] {
  return section.items.flatMap((item, index): Cell[] => {
    const key = itemKey(item, index);
    if (item.type === 'spacer') {
      return [
        {
          spacer: true,
          node: (
            <div
              key={key}
              aria-hidden
              className="form-layout-cell form-layout-spacer"
              style={{ '--span': item.span } as CellStyle}
            />
          ),
        },
      ];
    }
    const content = item.type === 'field' ? renderField(item.field) : renderBlock(item.block);
    if (content === null || content === undefined || content === false) {
      return [];
    }
    // 單號與欄位相同佔一格，其他系統區塊佔滿整列
    const wide = item.type === 'block' && item.block !== FORM_BLOCKS.recordNo;
    const span = item.type === 'field' ? item.span : 1;
    return [
      {
        spacer: false,
        node: (
          <div
            key={key}
            className={wide ? 'form-layout-cell form-layout-block' : 'form-layout-cell'}
            data-item={item.type === 'field' ? item.field : item.block}
            style={wide ? undefined : ({ '--span': span } as CellStyle)}
          >
            {content}
          </div>
        ),
      },
    ];
  });
}

function itemKey(item: FormItem, index: number): string {
  switch (item.type) {
    case 'field':
      return `field:${item.field}`;
    case 'block':
      return `block:${item.block}`;
    case 'spacer':
      return `spacer:${index}`;
  }
}

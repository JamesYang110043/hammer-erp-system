import { screen } from '@testing-library/react';
import type userEvent from '@testing-library/user-event';
import { vi } from 'vitest';

/**
 * jsdom 沒有版面，元素尺寸都是 0，dnd-kit 無法判斷「下一列」在哪裡。
 * 依表格列在 DOM 中的順序給每一列（與其中的元素）不同的位置，每列高 40px。
 */
export function layoutTableRows() {
  vi.spyOn(Element.prototype, 'getBoundingClientRect').mockImplementation(function (this: Element) {
    const row = this.closest('tr[data-row-key]');
    const rows = row ? [...document.querySelectorAll('tr[data-row-key]')] : [];
    const top = row ? rows.indexOf(row) * 40 : 0;
    const height = row ? 40 : 0;
    return {
      x: 0,
      y: top,
      top,
      left: 0,
      right: 800,
      bottom: top + height,
      width: 800,
      height,
      toJSON: () => ({}),
    } as DOMRect;
  });
}

/** 以鍵盤拖曳：空白鍵拿起、方向鍵往下移一列、空白鍵放下（dnd-kit 的鍵盤操作）。 */
export async function dragDownOneRow(user: ReturnType<typeof userEvent.setup>, label: string) {
  layoutTableRows();
  screen.getByRole('button', { name: `拖曳調整「${label}」的排序` }).focus();
  await user.keyboard(' ');
  await user.keyboard('{ArrowDown}');
  await user.keyboard(' ');
}

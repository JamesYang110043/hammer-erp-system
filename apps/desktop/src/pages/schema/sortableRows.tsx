import { HolderOutlined } from '@ant-design/icons';
import { DndContext, KeyboardSensor, PointerSensor, useSensor, useSensors } from '@dnd-kit/core';
import type { DragEndEvent } from '@dnd-kit/core';
import { restrictToVerticalAxis } from '@dnd-kit/modifiers';
import {
  SortableContext,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from '@dnd-kit/sortable';
import { CSS } from '@dnd-kit/utilities';
import { Button } from 'antd';
import { createContext, useContext, useMemo } from 'react';
import type { CSSProperties, HTMLAttributes, ReactNode } from 'react';

/**
 * 可拖曳排序的 antd Table（dnd-kit）：
 *
 * ```tsx
 * <SortableTableContext ids={rows.map((r) => r.id)} onDragEnd={...}>
 *   <Table components={{ body: { row: SortableRow } }} ... />
 * </SortableTableContext>
 * ```
 *
 * 把手欄位放 {@link DragHandle}。
 */

/** 拖曳把手需要的 listeners（由 {@link SortableRow} 提供給把手欄位）。 */
const DragHandleContext = createContext<{
  setActivatorNodeRef?: (el: HTMLElement | null) => void;
  listeners?: ReturnType<typeof useSortable>['listeners'];
  attributes?: ReturnType<typeof useSortable>['attributes'];
}>({});

export function DragHandle({ label }: { label: string }) {
  const { setActivatorNodeRef, listeners, attributes } = useContext(DragHandleContext);
  return (
    <Button
      type="text"
      size="small"
      icon={<HolderOutlined />}
      style={{ cursor: 'move' }}
      ref={setActivatorNodeRef}
      {...attributes}
      {...listeners}
      aria-label={`拖曳調整「${label}」的排序`}
    />
  );
}

/** 可拖曳的表格列（antd Table 的 `components.body.row`）。 */
export function SortableRow(
  props: HTMLAttributes<HTMLTableRowElement> & { 'data-row-key': string },
) {
  const {
    attributes,
    listeners,
    setNodeRef,
    setActivatorNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: props['data-row-key'] });
  const style: CSSProperties = {
    ...props.style,
    transform: CSS.Translate.toString(transform),
    transition,
    ...(isDragging ? { position: 'relative', zIndex: 10 } : {}),
  };
  const context = useMemo(
    () => ({ setActivatorNodeRef, listeners, attributes }),
    [setActivatorNodeRef, listeners, attributes],
  );
  return (
    <DragHandleContext.Provider value={context}>
      <tr {...props} ref={setNodeRef} style={style} />
    </DragHandleContext.Provider>
  );
}

/** 垂直拖曳排序的範圍。`ids` 為列的 `rowKey`，依目前順序。 */
export function SortableTableContext({
  ids,
  onDragEnd,
  children,
}: {
  ids: string[];
  onDragEnd: (event: DragEndEvent) => void;
  children: ReactNode;
}) {
  const sensors = useSensors(
    // 移動超過 4px 才算拖曳，避免點擊把手就觸發
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );
  return (
    <DndContext sensors={sensors} modifiers={[restrictToVerticalAxis]} onDragEnd={onDragEnd}>
      <SortableContext items={ids} strategy={verticalListSortingStrategy}>
        {children}
      </SortableContext>
    </DndContext>
  );
}

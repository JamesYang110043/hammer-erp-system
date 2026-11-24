import {
  DeleteOutlined,
  HolderOutlined,
  PlusOutlined,
  SaveOutlined,
  UndoOutlined,
} from '@ant-design/icons';
import {
  DndContext,
  KeyboardSensor,
  PointerSensor,
  closestCenter,
  pointerWithin,
  useDroppable,
  useSensor,
  useSensors,
} from '@dnd-kit/core';
import type {
  CollisionDetection,
  DragEndEvent,
  DragOverEvent,
  KeyboardCoordinateGetter,
} from '@dnd-kit/core';
import {
  SortableContext,
  rectSortingStrategy,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from '@dnd-kit/sortable';
import { CSS } from '@dnd-kit/utilities';
import {
  Alert,
  App,
  Button,
  Card,
  Checkbox,
  Empty,
  Form,
  Input,
  Modal,
  Popconfirm,
  Segmented,
  Select,
  Space,
  Tag,
  Tooltip,
  Typography,
  theme,
} from 'antd';
import { useEffect, useMemo, useReducer, useRef, useState } from 'react';
import type { CSSProperties } from 'react';
import { useBlocker, useLocation, useNavigate, useParams } from 'react-router';

import { StatusTag } from '../../components/StatusTag';
import { EmptyState } from '../../components/EmptyState';
import { Page } from '../../components/PageHeader';
import { apiErrorCode, errorMessage } from '../../api/client';
import { useFormLayout, useResetFormLayout, useSaveFormLayout } from '../../api/formLayouts';
import type { FormLayout } from '../../api/formLayouts';
import { useAllEntities, useEntity } from '../../api/schema';
import type { EntityDefinition, FieldDefinition } from '../../bindings';
import { ConflictAlert } from '../../components/ConflictAlert';
import {
  FORM_BLOCKS,
  MAX_FORM_COLUMNS,
  linesBlockDetail,
  resolveFormLayout,
  systemBlocks,
} from '../../records/formLayout';
import type { FormItem } from '../../records/formLayout';
import {
  designerReducer,
  dropId,
  findItem,
  placedKeys,
  sectionId,
  sectionKeyOfId,
  toConfig,
  toDesign,
} from '../../records/formLayoutDesigner';
import type {
  DesignItem,
  DesignSection,
  DesignerAction,
  DesignerState,
} from '../../records/formLayoutDesigner';
import { detailEntitiesOf } from '../../records/lines';
import { designerReturnPath } from '../../records/masterSplit';
import {
  SPLIT_DETAIL_PREVIEW_WIDTH,
  hasSplitPreview,
  layoutAppliesTo,
  sampleValue,
} from '../../records/formLayoutPreview';
import type { PreviewKind, PreviewWidth } from '../../records/formLayoutPreview';
import { FieldValue } from '../records/FieldValue';
import { FormLayoutRenderer } from '../records/FormLayoutRenderer';
import { PreviewItem, ReadonlyLayout } from '../records/ReadonlyLayout';

/** 系統區塊的名稱（§7.7）。 */
function blockLabel(code: string, details: readonly EntityDefinition[]): string {
  const detailKey = linesBlockDetail(code);
  if (detailKey !== undefined) {
    return `明細：${details.find((d) => d.key === detailKey)?.label ?? detailKey}`;
  }
  switch (code) {
    case FORM_BLOCKS.recordNo:
      return '單號';
    case FORM_BLOCKS.shipmentPicker:
      return '選擇出貨單';
    case FORM_BLOCKS.receiptPicker:
      return '選擇進貨單';
    case FORM_BLOCKS.allowanceAvailable:
      return '可折讓金額';
    case FORM_BLOCKS.usedLots:
      return '已用批號';
    default:
      return code;
  }
}

/** 設計器使用的資料類別資訊。 */
interface DesignerSchema {
  entity: EntityDefinition;
  fields: FieldDefinition[];
  /** 有效的明細子表，依排序。 */
  details: EntityDefinition[];
}

function itemLabel(item: FormItem, schema: DesignerSchema): string {
  switch (item.type) {
    case 'field':
      return schema.fields.find((f) => f.field_key === item.field)?.label ?? item.field;
    case 'block':
      return blockLabel(item.block, schema.details);
    case 'spacer':
      return '空白格';
  }
}

/**
 * 鍵盤拖曳時移到相鄰的對象：區塊只移到相鄰的區塊；項目只移到相鄰的項目，或空區塊的放置區
 *（不以整個區塊為目標，否則方向鍵會停在區塊上而不移動）。
 */
const keyboardCoordinates: KeyboardCoordinateGetter = (event, args) => {
  const containers = args.context.droppableContainers;
  const kind = args.context.active?.data.current?.kind as string | undefined;
  const keep = (data: Record<string, unknown> | undefined) =>
    kind === 'section'
      ? data?.kind === 'section'
      : data?.kind === 'item' || (data?.kind === 'drop' && data.empty === true);
  const filtered = new Proxy(containers, {
    get(target, prop) {
      if (prop === 'getEnabled') {
        return () => target.getEnabled().filter((c) => keep(c.data.current));
      }
      const value: unknown = Reflect.get(target, prop, target);
      return typeof value === 'function'
        ? (value as (...a: unknown[]) => unknown).bind(target)
        : value;
    },
  });
  return sortableKeyboardCoordinates(event, {
    ...args,
    context: { ...args.context, droppableContainers: filtered },
  });
};

/** 拖曳的對象：區塊只與區塊比對；項目與項目、區塊的放置區比對（優先指標所在的位置）。 */
const collisionDetection: CollisionDetection = (args) => {
  const kind = args.active.data.current?.kind as string | undefined;
  const droppableContainers = args.droppableContainers.filter((c) =>
    kind === 'section' ? c.data.current?.kind === 'section' : c.data.current?.kind !== 'section',
  );
  const scoped = { ...args, droppableContainers };
  const within = pointerWithin(scoped);
  if (within.length > 0) {
    // 指標同時在項目與放置區內時，以項目為準
    return [...within].sort(
      (a, b) =>
        Number(a.data?.droppableContainer?.data.current?.kind === 'drop') -
        Number(b.data?.droppableContainer?.data.current?.kind === 'drop'),
    );
  }
  if (args.pointerCoordinates) {
    return closestCenter(scoped);
  }
  // 鍵盤拖曳（沒有指標）：有項目的區塊以項目為目標，不以整個放置區為目標
  return closestCenter({
    ...scoped,
    droppableContainers: droppableContainers.filter(
      (c) => c.data.current?.kind !== 'drop' || c.data.current.empty === true,
    ),
  });
};

/**
 * 表單版面設計器（`/schema/:key/form-layout`，需要 `schema.manage`；docs/architecture.md §7.7）。
 *
 * 以拖曳（滑鼠或鍵盤）調整區塊與項目；系統區塊只能移動不能移除。儲存時帶 `version`，
 * 版本不符時提示重新載入。
 */
export function FormLayoutDesignerPage() {
  const { key = '' } = useParams();
  const navigate = useNavigate();
  const location = useLocation();
  const entity = useEntity(key);
  const entities = useAllEntities();
  const layout = useFormLayout(key);

  // 從分割檢視開啟時（#253）：返回與存檔後回到原本的畫面，保留選取的資料
  const returnTo = designerReturnPath(location.state, key);
  const back = () => void navigate(returnTo ?? `/schema/${key}`);
  const error = entity.error ?? entities.error ?? layout.error;
  if (error) {
    return (
      <Page title="表單版面">
        <Alert showIcon type="error" message={errorMessage(error)} />
      </Page>
    );
  }
  if (!entity.data || !entities.data || !layout.data) {
    return <Page title="表單版面" loading />;
  }
  const current = entity.data.entity;
  if (current.kind === 'detail') {
    return (
      <Page title="表單版面">
        <EmptyState
          kind="notFound"
          title="明細子表在主表的表單中編輯，沒有自己的表單版面"
          description="請到主表的欄位定義開啟表單版面"
        />
      </Page>
    );
  }
  const schema: DesignerSchema = {
    entity: current,
    fields: entity.data.fields.filter((f) => f.is_active),
    details: detailEntitiesOf(entities.data, current),
  };
  return (
    <Designer
      // 儲存、還原或重新載入後以新的版面重設
      key={`${key}:${layout.dataUpdatedAt}`}
      schema={schema}
      layout={layout.data}
      onBack={back}
      backLabel={returnTo ? `回到「${current.label}」` : `回到「${current.label}」的欄位定義`}
      onSaved={returnTo ? back : undefined}
      onReload={() => void layout.refetch()}
      reloading={layout.isFetching}
    />
  );
}

function Designer({
  schema,
  layout,
  onBack,
  backLabel,
  onSaved,
  onReload,
  reloading,
}: {
  schema: DesignerSchema;
  layout: FormLayout;
  onBack: () => void;
  backLabel: string;
  /** 存檔後（從分割檢視開啟時回到原本的畫面）。 */
  onSaved?: () => void;
  onReload: () => void;
  reloading: boolean;
}) {
  const entityKey = schema.entity.key;
  const { message } = App.useApp();
  const [state, dispatch] = useReducer(designerReducer, layout.config, toDesign);
  const [mode, setMode] = useState<'design' | 'preview'>('design');
  const [previewKind, setPreviewKind] = useState<PreviewKind>('form');
  const [previewWidth, setPreviewWidth] = useState<PreviewWidth>('full');
  const splitPreview = hasSplitPreview(entityKey);
  const [conflict, setConflict] = useState(false);
  const save = useSaveFormLayout(entityKey);
  const reset = useResetFormLayout(entityKey);

  const config = useMemo(() => toConfig(state), [state]);
  const dirty = useMemo(
    () => JSON.stringify(config) !== JSON.stringify(layout.config),
    [config, layout.config],
  );

  // 存檔成功後離開時不再提醒（快取更新前 dirty 仍為 true）
  const saved = useRef(false);
  const blocker = useBlocker(
    ({ currentLocation, nextLocation }) =>
      dirty && !saved.current && currentLocation.pathname !== nextLocation.pathname,
  );
  useEffect(() => {
    if (!dirty) {
      return;
    }
    const onBeforeUnload = (e: BeforeUnloadEvent) => e.preventDefault();
    window.addEventListener('beforeunload', onBeforeUnload);
    return () => window.removeEventListener('beforeunload', onBeforeUnload);
  }, [dirty]);

  const onError = (err: unknown) => {
    if (apiErrorCode(err) === 'CONFLICT') {
      setConflict(true);
    } else {
      message.error(errorMessage(err));
    }
  };

  const onSave = () =>
    save.mutateAsync({ version: layout.version ?? null, config }).then(() => {
      message.success('已儲存表單版面');
      if (onSaved) {
        saved.current = true;
        onSaved();
      }
    }, onError);

  const onReset = () => {
    if (layout.version === null) {
      dispatch({ type: 'reset', config: layout.config });
      return;
    }
    reset.mutate(layout.version, {
      onSuccess: () => void message.success('已還原成預設版面'),
      onError,
    });
  };

  const visibleFields = useMemo(
    () =>
      schema.fields
        .filter((f) => f.is_visible)
        .sort((a, b) => a.sort_order - b.sort_order || a.id.localeCompare(b.id)),
    [schema.fields],
  );
  const placed = placedKeys(state);
  const unplacedFields = visibleFields.filter((f) => !placed.fields.has(f.field_key));
  const unplacedBlocks = systemBlocks(
    entityKey,
    schema.details.map((d) => d.key),
  ).filter((b) => !placed.blocks.has(b.code));

  return (
    <Page
      onBack={onBack}
      backLabel={backLabel}
      title={`表單版面：${schema.entity.label}`}
      tags={
        (layout.is_default || dirty) && (
          <>
            {layout.is_default && <StatusTag tone="neutral">預設</StatusTag>}
            {dirty && <StatusTag tone="warning">未存檔</StatusTag>}
          </>
        )
      }
      actions={
        <Space>
          <Popconfirm
            title="還原成預設版面？"
            description="表單會回到依欄位定義排列的單欄畫面。"
            okText="還原"
            cancelText="取消"
            onConfirm={onReset}
            disabled={layout.is_default && !dirty}
          >
            <Button
              icon={<UndoOutlined />}
              disabled={layout.is_default && !dirty}
              loading={reset.isPending}
            >
              還原成預設
            </Button>
          </Popconfirm>
          <Button
            type="primary"
            icon={<SaveOutlined />}
            onClick={onSave}
            disabled={!dirty}
            loading={save.isPending}
          >
            儲存
          </Button>
        </Space>
      }
    >
      {conflict && (
        <ConflictAlert
          description="表單版面已被其他人修改。重新載入會取得最新的版面，並放棄目前未存檔的修改。"
          onReload={onReload}
          loading={reloading}
        />
      )}
      <Space direction="vertical" size="middle" style={{ width: '100%' }}>
        <Typography.Paragraph
          type="secondary"
          style={{ margin: 0 }}
          data-testid="layout-applies-to"
        >
          這份版面套用在：{layoutAppliesTo(entityKey).join('、')}。
        </Typography.Paragraph>
        <Space wrap size="middle">
          <Segmented
            value={mode}
            onChange={(v) => setMode(v as 'design' | 'preview')}
            options={[
              { value: 'design', label: '設計' },
              { value: 'preview', label: '預覽' },
            ]}
          />
          {mode === 'preview' && (
            <Segmented
              aria-label="預覽方式"
              value={previewKind}
              onChange={(v) => setPreviewKind(v as PreviewKind)}
              options={[
                { value: 'form', label: '編輯表單' },
                { value: 'readonly', label: '唯讀預覽' },
              ]}
            />
          )}
          {mode === 'preview' && splitPreview && (
            <Segmented
              aria-label="預覽寬度"
              value={previewWidth}
              onChange={(v) => setPreviewWidth(v as PreviewWidth)}
              options={[
                { value: 'full', label: '整頁' },
                { value: 'split', label: '分割檢視右側' },
              ]}
            />
          )}
        </Space>
        {mode === 'design' ? (
          <>
            <Typography.Paragraph type="secondary" style={{ margin: 0 }}>
              拖曳把手（或聚焦把手後按空白鍵、方向鍵）調整區塊與欄位的位置；系統區塊只能移動，不能移除。
            </Typography.Paragraph>
            <UnplacedPanel
              sections={state.sections}
              fields={unplacedFields}
              blocks={unplacedBlocks.map((b) => b.code)}
              schema={schema}
              dispatch={dispatch}
            />
            <SectionsEditor state={state} schema={schema} dispatch={dispatch} />
            <Button icon={<PlusOutlined />} onClick={() => dispatch({ type: 'addSection' })}>
              新增區塊
            </Button>
          </>
        ) : (
          <LayoutPreview
            state={state}
            schema={schema}
            fields={visibleFields}
            kind={previewKind}
            width={splitPreview ? previewWidth : 'full'}
          />
        )}
      </Space>
      <Modal
        open={blocker.state === 'blocked'}
        title="有未存檔的修改"
        okText="離開"
        okButtonProps={{ danger: true }}
        cancelText="留下"
        onOk={() => blocker.proceed?.()}
        onCancel={() => blocker.reset?.()}
      >
        離開後，目前的修改不會儲存。確定要離開？
      </Modal>
    </Page>
  );
}

/** 尚未放進版面的欄位與系統區塊：可以加入指定的區塊；不加入時表單會自動附加在最後。 */
function UnplacedPanel({
  sections,
  fields,
  blocks,
  schema,
  dispatch,
}: {
  sections: DesignSection[];
  fields: FieldDefinition[];
  blocks: string[];
  schema: DesignerSchema;
  dispatch: (action: DesignerAction) => void;
}) {
  const [target, setTarget] = useState<string>();
  const section = sections.find((s) => s.key === target) ?? sections[sections.length - 1];
  if (fields.length === 0 && blocks.length === 0) {
    return null;
  }
  const add = (item: FormItem) =>
    section && dispatch({ type: 'addItem', section: section.key, item });
  return (
    <Alert
      type="info"
      showIcon
      message="尚未放進版面"
      description={
        <Space direction="vertical" style={{ width: '100%' }}>
          <Typography.Text type="secondary">
            沒有放進版面的欄位與系統區塊會自動顯示在表單最後。
          </Typography.Text>
          <Space wrap>
            <span>加入到</span>
            <Select
              aria-label="加入到哪個區塊"
              size="small"
              style={{ minWidth: 160 }}
              value={section?.key}
              onChange={setTarget}
              options={sections.map((s, i) => ({
                value: s.key,
                label: s.title || `區塊 ${i + 1}`,
              }))}
            />
          </Space>
          <Space wrap>
            {fields.map((f) => (
              <Button
                key={f.field_key}
                size="small"
                icon={<PlusOutlined />}
                onClick={() => add({ type: 'field', field: f.field_key, span: 1 })}
              >
                {f.label}
              </Button>
            ))}
            {blocks.map((code) => (
              <Button
                key={code}
                size="small"
                icon={<PlusOutlined />}
                onClick={() => add({ type: 'block', block: code })}
              >
                {blockLabel(code, schema.details)}
              </Button>
            ))}
          </Space>
        </Space>
      }
    />
  );
}

function SectionsEditor({
  state,
  schema,
  dispatch,
}: {
  state: DesignerState;
  schema: DesignerSchema;
  dispatch: (action: DesignerAction) => void;
}) {
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
    useSensor(KeyboardSensor, { coordinateGetter: keyboardCoordinates }),
  );

  /** 拖曳 id 所在的區塊代碼（區塊、放置區或項目）。 */
  const sectionOf = (id: string): string | undefined => {
    const key = sectionKeyOfId(id);
    if (key !== undefined) {
      return key;
    }
    const found = findItem(state, id);
    return found ? state.sections[found.section]?.key : undefined;
  };

  // 項目拖到其他區塊時立即移過去（同一個區塊內的排序在放下時處理）
  const onDragOver = ({ active, over }: DragOverEvent) => {
    if (!over || active.data.current?.kind !== 'item') {
      return;
    }
    const id = String(active.id);
    const from = sectionOf(id);
    const to = sectionOf(String(over.id));
    if (!to || from === to) {
      return;
    }
    const overItem = findItem(state, String(over.id));
    const target = state.sections.find((s) => s.key === to);
    dispatch({
      type: 'moveItem',
      id,
      section: to,
      index: overItem ? overItem.index : (target?.items.length ?? 0),
    });
  };

  const onDragEnd = ({ active, over }: DragEndEvent) => {
    if (!over || active.id === over.id) {
      return;
    }
    const id = String(active.id);
    if (active.data.current?.kind === 'section') {
      const to = sectionOf(String(over.id));
      const index = state.sections.findIndex((s) => s.key === to);
      const key = sectionKeyOfId(id);
      if (key !== undefined && index >= 0) {
        dispatch({ type: 'moveSection', key, to: index });
      }
      return;
    }
    const overItem = findItem(state, String(over.id));
    const current = findItem(state, id);
    if (overItem && current && overItem.section === current.section) {
      const key = state.sections[overItem.section]!.key;
      dispatch({ type: 'moveItem', id, section: key, index: overItem.index });
    }
  };

  return (
    <DndContext
      sensors={sensors}
      collisionDetection={collisionDetection}
      onDragOver={onDragOver}
      onDragEnd={onDragEnd}
    >
      <SortableContext
        items={state.sections.map((s) => sectionId(s.key))}
        strategy={verticalListSortingStrategy}
      >
        <Space direction="vertical" size="middle" style={{ width: '100%' }}>
          {state.sections.map((section, i) => (
            <SectionEditor
              key={section.key}
              section={section}
              index={i}
              canRemove={state.sections.length > 1}
              schema={schema}
              dispatch={dispatch}
            />
          ))}
        </Space>
      </SortableContext>
    </DndContext>
  );
}

function SectionEditor({
  section,
  index,
  canRemove,
  schema,
  dispatch,
}: {
  section: DesignSection;
  index: number;
  canRemove: boolean;
  schema: DesignerSchema;
  dispatch: (action: DesignerAction) => void;
}) {
  const { token } = theme.useToken();
  const name = section.title || `區塊 ${index + 1}`;
  const {
    attributes,
    listeners,
    setNodeRef,
    setActivatorNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: sectionId(section.key), data: { kind: 'section' } });
  // 放置區：拖曳到區塊內的空白處（例如空的區塊）時放到最後
  const { setNodeRef: setDropRef, isOver } = useDroppable({
    id: dropId(section.key),
    data: { kind: 'drop', empty: section.items.length === 0 },
  });
  const style: CSSProperties = {
    transform: CSS.Translate.toString(transform),
    transition: transition,
    ...(isDragging ? { position: 'relative', zIndex: 10, opacity: 0.8 } : {}),
  };
  const update = (patch: Partial<DesignSection>) =>
    dispatch({ type: 'updateSection', key: section.key, patch });

  return (
    <section ref={setNodeRef} style={style} aria-label={`區塊：${name}`}>
      <Card
        size="small"
        title={
          <Space wrap>
            <Button
              type="text"
              size="small"
              icon={<HolderOutlined />}
              style={{ cursor: 'move' }}
              ref={setActivatorNodeRef}
              {...attributes}
              {...listeners}
              aria-label={`拖曳區塊「${name}」`}
            />
            <Input
              size="small"
              aria-label="區塊標題"
              placeholder="（不顯示標題）"
              maxLength={50}
              style={{ width: 200 }}
              value={section.title}
              onChange={(e) => update({ title: e.target.value })}
            />
            <Select
              size="small"
              aria-label="欄數"
              style={{ width: 90 }}
              value={section.columns}
              onChange={(columns) => update({ columns })}
              options={Array.from({ length: MAX_FORM_COLUMNS }, (_, i) => ({
                value: i + 1,
                label: `${i + 1} 欄`,
              }))}
            />
            <Tooltip title={section.title.trim() ? undefined : '可收折的區塊需要標題'}>
              <Checkbox
                checked={section.collapsible}
                disabled={!section.title.trim() && !section.collapsible}
                onChange={(e) => update({ collapsible: e.target.checked })}
              >
                可收折
              </Checkbox>
            </Tooltip>
            <Checkbox
              checked={section.collapsed}
              disabled={!section.collapsible}
              onChange={(e) => update({ collapsed: e.target.checked })}
            >
              預設收折
            </Checkbox>
          </Space>
        }
        extra={
          <Space>
            <Button
              size="small"
              icon={<PlusOutlined />}
              onClick={() => dispatch({ type: 'addSpacer', section: section.key })}
            >
              空白格
            </Button>
            <Popconfirm
              title={`刪除區塊「${name}」？`}
              description="其中的欄位會移到「尚未放進版面」，系統區塊移到第一個區塊。"
              okText="刪除"
              okButtonProps={{ danger: true }}
              cancelText="取消"
              onConfirm={() => dispatch({ type: 'removeSection', key: section.key })}
              disabled={!canRemove}
            >
              <Button
                size="small"
                danger
                icon={<DeleteOutlined />}
                disabled={!canRemove}
                aria-label={`刪除區塊「${name}」`}
              />
            </Popconfirm>
          </Space>
        }
      >
        <SortableContext items={section.items.map((i) => i.id)} strategy={rectSortingStrategy}>
          <div
            ref={setDropRef}
            style={{
              display: 'grid',
              gridTemplateColumns: `repeat(${section.columns}, minmax(0, 1fr))`,
              gap: 8,
              minHeight: 48,
              padding: 4,
              borderRadius: 6,
              outline: isOver ? `2px dashed ${token.colorPrimary}` : undefined,
            }}
          >
            {section.items.length === 0 && (
              <Empty
                image={Empty.PRESENTED_IMAGE_SIMPLE}
                description="把欄位拖曳到這裡"
                style={{ gridColumn: '1 / -1', margin: 0 }}
              />
            )}
            {section.items.map((item) => (
              <ItemChip
                key={item.id}
                item={item}
                columns={section.columns}
                schema={schema}
                dispatch={dispatch}
              />
            ))}
          </div>
        </SortableContext>
      </Card>
    </section>
  );
}

function ItemChip({
  item,
  columns,
  schema,
  dispatch,
}: {
  item: DesignItem;
  columns: number;
  schema: DesignerSchema;
  dispatch: (action: DesignerAction) => void;
}) {
  const { token } = theme.useToken();
  const label = itemLabel(item, schema);
  const {
    attributes,
    listeners,
    setNodeRef,
    setActivatorNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: item.id, data: { kind: 'item' } });
  const wide = item.type === 'block' && item.block !== FORM_BLOCKS.recordNo;
  const hidden =
    item.type === 'field' &&
    schema.fields.find((f) => f.field_key === item.field)?.is_visible === false;
  const style: CSSProperties = {
    gridColumn: wide ? '1 / -1' : `span ${item.type === 'block' ? 1 : item.span}`,
    transform: CSS.Translate.toString(transform),
    transition: transition,
    display: 'flex',
    alignItems: 'center',
    gap: 4,
    minWidth: 0,
    padding: '4px 8px',
    border: `1px ${item.type === 'spacer' ? 'dashed' : 'solid'} ${token.colorBorder}`,
    borderRadius: 6,
    background: item.type === 'block' ? token.colorPrimaryBg : undefined,
    ...(isDragging ? { position: 'relative', zIndex: 10, opacity: 0.8 } : {}),
  };
  return (
    <div ref={setNodeRef} style={style} data-design-item={item.id}>
      <Button
        type="text"
        size="small"
        icon={<HolderOutlined />}
        style={{ cursor: 'move' }}
        ref={setActivatorNodeRef}
        {...attributes}
        {...listeners}
        aria-label={`拖曳「${label}」`}
      />
      <Typography.Text
        ellipsis
        style={{ flex: 1, minWidth: 0 }}
        type={hidden ? 'secondary' : undefined}
      >
        {label}
      </Typography.Text>
      {item.type === 'block' && <Tag style={{ marginInlineEnd: 0 }}>系統區塊</Tag>}
      {hidden && <Tag style={{ marginInlineEnd: 0 }}>不顯示</Tag>}
      {item.type !== 'block' && columns > 1 && (
        <Select
          size="small"
          aria-label={`「${label}」的寬度`}
          style={{ width: 76 }}
          value={item.span}
          onChange={(span) => dispatch({ type: 'setSpan', id: item.id, span })}
          options={Array.from({ length: columns }, (_, i) => ({
            value: i + 1,
            label: `${i + 1} 欄`,
          }))}
        />
      )}
      {item.type !== 'block' && (
        <Button
          type="text"
          size="small"
          icon={<DeleteOutlined />}
          aria-label={`移除「${label}」`}
          onClick={() => dispatch({ type: 'removeItem', id: item.id })}
        />
      )}
    </div>
  );
}

/**
 * 預覽：以實際的表單版面元件呈現。
 *
 * - 編輯表單：欄位為停用的輸入框
 * - 唯讀預覽：與分割檢視右側的 `RecordPreview` 共用 {@link ReadonlyLayout}，值為範例
 * - 系統區塊（單號以外）為佔位；寬度為「分割檢視右側」時以 1440px 視窗的右側寬度呈現
 */
function LayoutPreview({
  state,
  schema,
  fields,
  kind,
  width,
}: {
  state: DesignerState;
  schema: DesignerSchema;
  fields: FieldDefinition[];
  kind: PreviewKind;
  width: PreviewWidth;
}) {
  const { token } = theme.useToken();
  const config = useMemo(
    () =>
      resolveFormLayout(toConfig(state), {
        entityKey: schema.entity.key,
        fieldKeys: fields.map((f) => f.field_key),
        detailKeys: schema.details.map((d) => d.key),
      }),
    [state, schema, fields],
  );
  const byKey = new Map(fields.map((f) => [f.field_key, f]));
  const placeholder = (code: string) => (
    <div
      style={{
        marginBottom: kind === 'form' ? 24 : 16,
        padding: '16px 12px',
        border: `1px dashed ${token.colorBorder}`,
        borderRadius: 6,
        textAlign: 'center',
      }}
    >
      <Typography.Text type="secondary">{blockLabel(code, schema.details)}</Typography.Text>
    </div>
  );
  const content =
    kind === 'form' ? (
      <Form layout="vertical" disabled>
        <FormLayoutRenderer
          config={config}
          renderField={(key) => {
            const field = byKey.get(key);
            return field ? (
              <Form.Item label={field.label} required={field.is_required}>
                <Input />
              </Form.Item>
            ) : null;
          }}
          renderBlock={(code) =>
            code === FORM_BLOCKS.recordNo ? (
              <Form.Item label="單號">
                <Input style={{ maxWidth: 320 }} placeholder="存檔後產生" />
              </Form.Item>
            ) : (
              placeholder(code)
            )
          }
        />
      </Form>
    ) : (
      <ReadonlyLayout
        config={config}
        fields={fields}
        renderValue={(field) => <SampleValue field={field} />}
        renderBlock={(code) =>
          code === FORM_BLOCKS.recordNo ? (
            <PreviewItem label="單號">
              <Typography.Text type="secondary">單號範例</Typography.Text>
            </PreviewItem>
          ) : (
            placeholder(code)
          )
        }
      />
    );
  return (
    <section aria-label="表單預覽" data-preview-kind={kind} data-preview-width={width}>
      {width === 'split' ? (
        // 與分割檢視右側相同的卡片與寬度
        <Card
          data-testid="split-preview-frame"
          style={{ width: SPLIT_DETAIL_PREVIEW_WIDTH, maxWidth: '100%' }}
        >
          {content}
        </Card>
      ) : (
        content
      )}
    </section>
  );
}

/** 唯讀預覽的範例值：依欄位型態格式化；關聯、附件顯示佔位文字。 */
function SampleValue({ field }: { field: FieldDefinition }) {
  const value = sampleValue(field);
  if (value === undefined) {
    return (
      <Typography.Text type="secondary">
        {field.data_type === 'file' ? '附件範例' : `${field.label}範例`}
      </Typography.Text>
    );
  }
  return <FieldValue field={field} value={value} />;
}

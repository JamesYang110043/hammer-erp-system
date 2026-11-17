import type {
  EntityDefinition,
  EntityKind,
  EntityOrderItem,
  UpdateEntityRequest,
} from '../bindings';

export const KIND_LABELS: Record<EntityKind, string> = {
  master: '主檔',
  document: '單據',
  detail: '明細子表',
};

/** 系統主檔（商品、客戶等，docs/architecture.md §8.6）在選單與選項中的分組名稱。 */
export const MASTER_DATA_LABEL = '基本資料';

/** 選單與管理畫面的分組順序（明細子表從所屬的主表管理，不分組顯示）。 */
export const MENU_KINDS = ['master', 'document'] as const satisfies readonly EntityKind[];
export type MenuKind = (typeof MENU_KINDS)[number];

export interface EntityGroup {
  kind: MenuKind;
  label: string;
  entities: EntityDefinition[];
}

function bySortOrder(a: EntityDefinition, b: EntityDefinition): number {
  // 伺服器依 `sort_order, id` 排序；id 為 uuid v7，字串順序即建立順序
  return a.sort_order - b.sort_order || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0);
}

/** 依種類分組（主檔、單據），組內依選單排序。明細子表不列出。 */
export function groupEntities(entities: readonly EntityDefinition[]): EntityGroup[] {
  return MENU_KINDS.map((kind) => ({
    kind,
    label: KIND_LABELS[kind],
    entities: entities.filter((e) => e.kind === kind).sort(bySortOrder),
  }));
}

/** 主表的明細子表，依排序。 */
export function detailEntitiesOf(
  entities: readonly EntityDefinition[],
  parentId: string,
): EntityDefinition[] {
  return entities
    .filter((e) => e.kind === 'detail' && e.parent_entity_id === parentId)
    .sort(bySortOrder);
}

/**
 * 拖曳後的新順序 → 批次排序的請求項目：`sort_order` 依組內位置（從 0 開始），
 * 只列出排序有改變的資料類別。
 */
export function reorderItems(ordered: readonly EntityDefinition[]): EntityOrderItem[] {
  return ordered.flatMap((e, index) =>
    e.sort_order === index ? [] : [{ key: e.key, version: e.version, sort_order: index }],
  );
}

/** 修改資料類別的請求（整筆取代）：以目前的值為基礎，套用 `patch`。 */
export function toUpdateEntityRequest(
  entity: EntityDefinition,
  patch: Partial<Omit<UpdateEntityRequest, 'version' | 'key'>> = {},
): UpdateEntityRequest {
  return {
    version: entity.version,
    key: entity.key,
    label: entity.label,
    icon: entity.icon ?? null,
    numbering_pattern: entity.numbering_pattern ?? null,
    title_template: entity.title_template ?? null,
    sort_order: entity.sort_order,
    is_active: entity.is_active,
    ...patch,
  };
}

/** 空白字串 → `null`；其他去除前後空白。 */
export function blankToNull(value: string | null | undefined): string | null {
  const trimmed = value?.trim();
  return trimmed ? trimmed : null;
}

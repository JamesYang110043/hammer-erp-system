import { describe, expect, it } from 'vitest';

import type { EntityDefinition } from '../bindings';
import { REPAIR } from '../test/schemaFixtures';
import {
  blankToNull,
  detailEntitiesOf,
  groupEntities,
  reorderItems,
  toUpdateEntityRequest,
} from './entities';

function entity(patch: Partial<EntityDefinition>): EntityDefinition {
  return { ...REPAIR, ...patch };
}

const A = entity({ id: '01-a', key: 'a', sort_order: 2 });
const B = entity({ id: '01-b', key: 'b', sort_order: 0 });
const C = entity({ id: '01-c', key: 'c', sort_order: 0 });
const DOC = entity({ id: '01-d', key: 'doc', kind: 'document' });
const LINE = entity({ id: '01-e', key: 'line', kind: 'detail', parent_entity_id: DOC.id });

describe('groupEntities', () => {
  it('groups_by_kind_and_sorts_without_details', () => {
    const groups = groupEntities([A, DOC, LINE, C, B]);
    expect(groups.map((g) => [g.label, g.entities.map((e) => e.key)])).toEqual([
      // 排序相同時依 id（建立順序）
      ['主檔', ['b', 'c', 'a']],
      ['單據', ['doc']],
    ]);
  });
});

describe('detailEntitiesOf', () => {
  it('lists_details_of_parent', () => {
    expect(detailEntitiesOf([A, DOC, LINE], DOC.id)).toEqual([LINE]);
    expect(detailEntitiesOf([A, DOC, LINE], A.id)).toEqual([]);
  });
});

describe('reorderItems', () => {
  it('assigns_positions_and_lists_only_changed', () => {
    // 新順序 b, c, a：b、c 已經是 0 → c 改為 1，a 改為 2（原本就是 2）
    expect(reorderItems([B, C, A])).toEqual([{ key: 'c', version: C.version, sort_order: 1 }]);
    expect(reorderItems([A, B])).toEqual([
      { key: 'a', version: A.version, sort_order: 0 },
      { key: 'b', version: B.version, sort_order: 1 },
    ]);
  });
});

describe('toUpdateEntityRequest', () => {
  it('keeps_current_values_and_applies_patch', () => {
    const e = entity({ numbering_pattern: 'R{000}', title_template: '{record_no}', version: 7 });
    expect(toUpdateEntityRequest(e, { is_active: false })).toEqual({
      version: 7,
      key: 'repair',
      label: '維修單',
      icon: 'ToolOutlined',
      numbering_pattern: 'R{000}',
      title_template: '{record_no}',
      sort_order: 0,
      is_active: false,
    });
  });
});

describe('blankToNull', () => {
  it('trims_and_maps_blank_to_null', () => {
    expect(blankToNull('  ')).toBeNull();
    expect(blankToNull(undefined)).toBeNull();
    expect(blankToNull(' {name} ')).toBe('{name}');
  });
});

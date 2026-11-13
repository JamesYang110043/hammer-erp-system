import dayjs from 'dayjs';
import { describe, expect, it } from 'vitest';

import { AUDIT_ACTIONS } from '../../bindings';
import { AUDIT_ACTION_LABELS, actionLabel, buildAuditLogQuery, entityLabel } from './auditQuery';

describe('buildAuditLogQuery', () => {
  it('empty_filters_only_send_paging', () => {
    expect(buildAuditLogQuery({}, 1, 50)).toEqual({
      from: null,
      to: null,
      user_id: null,
      action: null,
      entity_key: null,
      record_id: null,
      page: 1,
      page_size: 50,
    });
  });

  it('date_range_is_taipei_days_with_exclusive_end', () => {
    const q = buildAuditLogQuery({ dateRange: [dayjs('2026-11-01'), dayjs('2026-11-03')] }, 2, 20);
    expect(q.from).toBe('2026-11-01T00:00:00+08:00');
    // 含結束日：伺服器的 `to` 不含，所以是隔天 00:00
    expect(q.to).toBe('2026-11-04T00:00:00+08:00');
    expect([q.page, q.page_size]).toEqual([2, 20]);
  });

  it('end_of_month_rolls_over', () => {
    const q = buildAuditLogQuery({ dateRange: [null, dayjs('2026-12-31')] }, 1, 50);
    expect(q.from).toBeNull();
    expect(q.to).toBe('2027-01-01T00:00:00+08:00');
  });

  it('user_and_action', () => {
    const q = buildAuditLogQuery({ userId: 'u1', action: 'login_failed' }, 1, 50);
    expect(q.user_id).toBe('u1');
    expect(q.action).toBe('login_failed');
  });
});

describe('labels', () => {
  it('every_action_has_a_label', () => {
    for (const action of AUDIT_ACTIONS) {
      expect(AUDIT_ACTION_LABELS[action]).toBeTruthy();
    }
    expect(actionLabel('password_change')).toBe('變更密碼');
    expect(actionLabel('unknown')).toBe('unknown');
    expect(entityLabel('user')).toBe('帳號');
    expect(entityLabel('customers_x')).toBe('customers_x');
  });
});

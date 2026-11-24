import { Form, Input, Select, Typography, theme } from 'antd';
import type { FormInstance } from 'antd';
import type { TextAreaRef } from 'antd/es/input/TextArea';
import { useMemo, useRef } from 'react';

import { FULL_ROW } from '../../components/FormGrid';
import { useAllEntities, useEntity, useEntityDetails } from '../../api/schema';
import type { FieldDefinition, FormulaSyntaxError } from '../../bindings';
import { detailEntitiesOf } from '../../schema/entities';
import type { FieldFormValues } from '../../schema/fieldForm';
import { DATA_TYPE_LABELS, FORMULA_RESULT_TYPES } from '../../schema/fieldSettings';
import { formulaRefGroups, insertText, splitAtSpan } from '../../schema/formula';

/**
 * 公式欄位的設定：公式輸入區（插入欄位、即時語法檢查）與結果型態（docs/architecture.md §7.3、§7.6）。
 *
 * - 語法錯誤（`syntaxError`，由上層以 `useFormulaSyntax` 檢查）顯示在輸入區下方並標示位置
 * - 伺服器的 `422`（引用錯誤、型態不符、循環路徑）以表單錯誤顯示在輸入區下方（`applyFormError`）
 * - 小數位數依結果型態，由 `TypeSettingsFields` 顯示
 *
 * 必須放在 antd `Form` 內；欄位名稱與 `FieldFormValues` 相同。
 */
export function FormulaSettings({
  form,
  entityKey,
  selfKey,
  syntaxError,
  disabled = false,
}: {
  form: FormInstance<FieldFormValues>;
  entityKey: string;
  /** 正在編輯的欄位代碼（不列入插入清單）；新增時為 `undefined`。 */
  selfKey?: string;
  syntaxError?: FormulaSyntaxError;
  disabled?: boolean;
}) {
  const textArea = useRef<TextAreaRef>(null);
  const groups = useFormulaRefGroups(entityKey, selfKey);
  const formula = Form.useWatch('formula', form) ?? '';

  const insert = (text: string) => {
    const el = textArea.current?.resizableTextArea?.textArea;
    const current = (form.getFieldValue('formula') as string | null | undefined) ?? '';
    const next = insertText(current, text, el?.selectionStart, el?.selectionEnd);
    // 經過 setFields 才會觸發欄位的驗證與清除伺服器錯誤
    form.setFields([{ name: 'formula', value: next.value, errors: [] }]);
    if (el) {
      el.focus();
      requestAnimationFrame(() => el.setSelectionRange(next.cursor, next.cursor));
    }
  };

  return (
    <>
      <Form.Item
        className={FULL_ROW}
        label="公式"
        name="formula"
        rules={[{ required: true, whitespace: true, message: '請輸入公式' }]}
        extra={
          syntaxError ? (
            <SyntaxErrorView formula={formula} error={syntaxError} />
          ) : (
            '例如 {qty} * {unit_price}、SUM(lines.amount)；欄位以代碼引用，修改顯示名稱不影響公式'
          )
        }
        validateStatus={syntaxError ? 'error' : undefined}
      >
        <Input.TextArea
          ref={textArea}
          rows={3}
          autoComplete="off"
          spellCheck={false}
          disabled={disabled}
          style={{ fontFamily: 'monospace' }}
        />
      </Form.Item>
      <Form.Item label="插入欄位" htmlFor="formula-insert">
        <Select<string>
          id="formula-insert"
          placeholder="選擇欄位，插入到公式的游標位置"
          value={null}
          showSearch
          optionFilterProp="label"
          disabled={disabled}
          options={groups.map((g) => ({
            label: g.label,
            title: g.label,
            options: g.options.map((o) => ({ ...o, title: o.label })),
          }))}
          onSelect={insert}
          notFoundContent="沒有可以引用的欄位"
          // 選項不多，不使用虛擬捲動（全部選項都在 DOM 中）
          virtual={false}
        />
      </Form.Item>
      <Form.Item
        label="結果型態"
        name="formula_result_type"
        rules={[{ required: true, message: '請選擇結果型態' }]}
      >
        <Select
          disabled={disabled}
          placeholder="公式計算結果的型態"
          virtual={false}
          options={FORMULA_RESULT_TYPES.map((t) => ({ value: t, label: DATA_TYPE_LABELS[t] }))}
        />
      </Form.Item>
    </>
  );
}

/** 語法錯誤：訊息與錯誤位置（以標示顯示公式中的錯誤字元）。 */
function SyntaxErrorView({ formula, error }: { formula: string; error: FormulaSyntaxError }) {
  const { before, marked, after } = splitAtSpan(formula, error.start, error.end);
  const { token } = theme.useToken();
  return (
    <div role="alert">
      <Typography.Text type="danger">
        第 {error.start + 1} 個字元：{error.message}
      </Typography.Text>
      <div
        data-testid="formula-error-position"
        style={{ fontFamily: 'monospace', whiteSpace: 'pre-wrap', wordBreak: 'break-all' }}
      >
        {before}
        <mark
          style={{
            background: token.colorErrorBg,
            color: token.colorError,
            borderBottom: '2px solid currentColor',
            padding: 0,
          }}
        >
          {/* 位置在結尾（公式不完整）時以空白標示 */}
          {marked || ' '}
        </mark>
        {after}
      </div>
    </div>
  );
}

/** 插入清單的欄位：本筆、關聯欄位的目標、明細子表（只有主表）。 */
function useFormulaRefGroups(entityKey: string, selfKey: string | undefined) {
  const entity = useEntity(entityKey);
  const all = useAllEntities();
  const current = entity.data?.entity;
  const fields = useMemo(() => entity.data?.fields ?? [], [entity.data]);

  const relationFields = fields.filter((f) => f.is_active && f.data_type === 'relation');
  const relationEntities = (all.data ?? []).filter((e) =>
    relationFields.some((f) => f.relation_entity_id === e.id),
  );
  const detailEntities =
    current && current.kind !== 'detail'
      ? detailEntitiesOf(all.data ?? [], current.id).filter((e) => e.is_active)
      : [];

  const relationDetails = useEntityDetails(all.data ? relationEntities : undefined);
  const detailDetails = useEntityDetails(all.data ? detailEntities : undefined);

  const relationTargets = new Map<string, FieldDefinition[]>();
  for (const f of relationFields) {
    const target = relationDetails.data?.find((d) => d.entity.id === f.relation_entity_id);
    if (target) {
      relationTargets.set(f.field_key, target.fields);
    }
  }

  return formulaRefGroups({
    fields,
    selfKey,
    relationTargets,
    details: detailDetails.data ?? [],
  });
}

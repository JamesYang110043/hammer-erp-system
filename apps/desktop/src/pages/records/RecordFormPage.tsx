import {
  ArrowLeftOutlined,
  CheckCircleOutlined,
  ReloadOutlined,
  SaveOutlined,
  StopOutlined,
} from '@ant-design/icons';
import {
  Alert,
  App,
  Button,
  Card,
  Empty,
  Form,
  Input,
  Modal,
  Space,
  Tag,
  Tooltip,
  Typography,
} from 'antd';
import { useEffect, useMemo, useRef, useState } from 'react';
import { useBlocker, useNavigate, useParams } from 'react-router';

import { apiErrorCode, errorMessage } from '../../api/client';
import {
  useCreateRecord,
  useDocumentAction,
  useFormulaPreview,
  useRecord,
  useUpdateRecord,
} from '../../api/records';
import { useEntities, useEntity, useEntityDetails } from '../../api/schema';
import { usePermission } from '../../auth/permission';
import { PERMISSIONS } from '../../bindings';
import type {
  EntityDefinition,
  EntityDetail,
  FieldDefinition,
  Record as DataRecord,
} from '../../bindings';
import { recordListPath, recordPath } from '../../modules';
import {
  DOCUMENT_ACTION_LABELS,
  availableAction,
  documentActionError,
  documentStatus,
  isEditableDocument,
  isSystemDocument,
} from '../../records/documents';
import type { DocumentAction, DocumentActionError } from '../../records/documents';
import {
  baselineData,
  buildCreateData,
  buildUpdateData,
  changedFieldKeys,
  formFields,
  initialFormValues,
  recordFieldErrors,
  validateFieldValue,
} from '../../records/form';
import type { RecordFormValues } from '../../records/form';
import { buildPreviewRequest, formulaCell, formulaFields } from '../../records/formulaPreview';
import type { PreviewTable } from '../../records/formulaPreview';
import {
  LINES_FORM_KEY,
  buildCreateLines,
  buildUpdateLines,
  detailEntitiesOf,
  initialLinesValues,
  lineErrorPath,
  lineErrorText,
  linesBaseline,
  linesChanged,
  recordLineErrors,
  showsRecordNo,
} from '../../records/lines';
import type { DetailTable, LineFormValues } from '../../records/lines';
import { entityIcon } from '../schema/entityIcons';
import { formatTaipei } from '../../utils/time';
import { DocumentStatusTag } from './DocumentStatusTag';
import { FormulaValue } from './FieldValue';
import { LinesTable } from './LinesTable';
import type { LineRefs } from './LinesTable';
import { RecordFieldInput } from './RecordFieldInput';

/** 樂觀鎖定衝突（409）的提示（docs/architecture.md §9.4）。 */
export const RECORD_CONFLICT_MESSAGE = '資料已被其他人修改';

/** 新增資料時單號欄位的提示（單號由伺服器在存檔時產生）。 */
export const RECORD_NO_PENDING = '存檔後產生';

/** 確認對話框的說明（docs/architecture.md §8.2）。 */
const ACTION_CONFIRM: Record<DocumentAction, string> = {
  post: '過帳後會依明細異動庫存，單據不能再修改或刪除。',
  void: '作廢會反向處理這張單據的庫存異動，作廢後不能再恢復。',
};

/** 表單有未存檔的修改時，過帳、作廢按鈕的提示。 */
export const SAVE_BEFORE_ACTION = '請先儲存目前的修改';

type FormValues = RecordFormValues & { [LINES_FORM_KEY]?: Record<string, LineFormValues[]> };

/** 明細列目前的關聯與附件，鍵為明細 id。 */
function lineRefs(record?: DataRecord): LineRefs {
  const refs: LineRefs = { relations: {}, attachments: {} };
  for (const lines of Object.values(record?.lines ?? {})) {
    for (const line of lines) {
      refs.relations[line.id] = line.relations ?? {};
      refs.attachments[line.id] = line.attachments ?? {};
    }
  }
  return refs;
}

/** 明細列的既有資料（公式欄位顯示上一次存檔的值用），鍵為明細 id。 */
function savedLines(record?: DataRecord): Map<string, DataRecord> {
  return new Map(
    Object.values(record?.lines ?? {})
      .flat()
      .map((line) => [line.id, line]),
  );
}

/**
 * 新增或編輯一筆資料的表單。`record` 為 `undefined` 時是新增。
 *
 * 公式欄位唯讀：輸入其他欄位或明細時，以 Tauri command 即時預覽（`records/formulaPreview`）；
 * 預覽有錯誤時顯示錯誤提示，但不阻擋存檔（存檔時以伺服器計算為準，§7.6）。
 *
 * 父元件在資料重新載入（存檔成功、衝突後重新載入）時以 `key` 重設本元件。
 */
function RecordForm({
  entity,
  fields,
  details,
  detailDefs,
  record,
  onReload,
}: {
  entity: EntityDefinition;
  fields: FieldDefinition[];
  /** 明細子表（欄位已依表單規則篩選）。 */
  details: DetailTable[];
  /** 明細子表完整的欄位定義（公式預覽用）。 */
  detailDefs: EntityDetail[];
  record?: DataRecord;
  onReload: () => void;
}) {
  const entityKey = entity.key;
  const navigate = useNavigate();
  const { message, modal } = App.useApp();
  const [form] = Form.useForm<FormValues>();
  const canWrite = usePermission(PERMISSIONS.RECORD_WRITE);
  const canPost = usePermission(PERMISSIONS.RECORD_POST);
  // 系統單據：只有草稿可以修改；過帳、作廢依狀態顯示（伺服器同樣檢查）
  const isDocument = isSystemDocument(entity);
  const status = isDocument ? documentStatus(record) : undefined;
  const editable = canWrite && isEditableDocument(entity, record);
  const action = record && canPost ? availableAction(status) : undefined;
  const postRecord = useDocumentAction(entityKey, 'post');
  const voidRecord = useDocumentAction(entityKey, 'void');
  const [actionError, setActionError] = useState<DocumentActionError>();
  const createRecord = useCreateRecord(entityKey);
  const updateRecord = useUpdateRecord(entityKey);
  const saving = createRecord.isPending || updateRecord.isPending;

  const visible = useMemo(() => formFields(fields), [fields]);
  // 表單上的欄位與公式欄位，依欄位排序
  const displayed = useMemo(
    () => [...visible, ...formulaFields(fields)].sort((a, b) => a.sort_order - b.sort_order),
    [visible, fields],
  );
  const initialValues = useMemo<FormValues>(
    () => ({
      ...initialFormValues(visible, record?.data),
      [LINES_FORM_KEY]: initialLinesValues(details, record),
    }),
    [visible, details, record],
  );
  const baseline = useMemo(() => baselineData(visible, record?.data), [visible, record]);
  const lineBaseline = useMemo(() => linesBaseline(details, record), [details, record]);
  const refs = useMemo(() => lineRefs(record), [record]);
  const savedLineById = useMemo(() => savedLines(record), [record]);

  // 公式預覽：表單目前的值（onValuesChange 時更新）
  const [current, setCurrent] = useState<FormValues>(initialValues);
  const previewTables = useMemo(() => {
    const shown = new Map(details.map((d) => [d.entity.key, d.fields]));
    return {
      master: { fields, shown: visible } satisfies PreviewTable,
      details: new Map(
        detailDefs.map((d) => [
          d.entity.key,
          { fields: d.fields, shown: shown.get(d.entity.key) ?? [] } satisfies PreviewTable,
        ]),
      ),
    };
  }, [fields, visible, details, detailDefs]);
  const previewRequest = useMemo(
    () =>
      buildPreviewRequest(
        previewTables.master,
        previewTables.details,
        current,
        current[LINES_FORM_KEY],
        record,
      ),
    [previewTables, current, record],
  );
  const preview = useFormulaPreview(previewRequest).data;

  const [dirty, setDirty] = useState(false);
  const [conflict, setConflict] = useState(false);
  // 離開提醒在導覽當下讀取；存檔成功後要立刻導覽，不能等 state 更新
  const dirtyRef = useRef(false);
  const markDirty = (value: boolean) => {
    dirtyRef.current = value;
    setDirty(value);
  };

  const blocker = useBlocker(
    ({ currentLocation, nextLocation }) =>
      dirtyRef.current && currentLocation.pathname !== nextLocation.pathname,
  );

  // 關閉視窗或重新整理時也提醒
  useEffect(() => {
    if (!dirty) {
      return;
    }
    const onBeforeUnload = (e: BeforeUnloadEvent) => e.preventDefault();
    window.addEventListener('beforeunload', onBeforeUnload);
    return () => window.removeEventListener('beforeunload', onBeforeUnload);
  }, [dirty]);

  const onValuesChange = () => {
    const values = form.getFieldsValue(true) as FormValues;
    setCurrent(values);
    markDirty(
      changedFieldKeys(visible, values, baseline).length > 0 ||
        linesChanged(details, values[LINES_FORM_KEY], lineBaseline),
    );
  };

  const onSaveError = (err: unknown) => {
    if (apiErrorCode(err) === 'CONFLICT') {
      setConflict(true);
      return;
    }
    const fieldErrors = recordFieldErrors(err) ?? [];
    const lineErrors = recordLineErrors(err) ?? [];
    if (fieldErrors.length > 0 || lineErrors.length > 0) {
      const shown = new Set(visible.map((f) => f.field_key));
      const lines = (form.getFieldValue(LINES_FORM_KEY) ?? {}) as Record<string, LineFormValues[]>;
      const rowCounts = Object.fromEntries(
        Object.entries(lines).map(([key, rows]) => [key, rows.length]),
      );
      const cells: { name: (string | number)[]; errors: string[] }[] = [];
      const others: string[] = [];
      for (const e of fieldErrors) {
        if (shown.has(e.field_key)) {
          cells.push({ name: [e.field_key], errors: [e.message] });
        } else {
          others.push(`${e.field_key}：${e.message}`);
        }
      }
      for (const e of lineErrors) {
        const path = lineErrorPath(e, details, rowCounts);
        if (path) {
          cells.push({ name: path, errors: [e.message] });
        } else {
          others.push(lineErrorText(e, details));
        }
      }
      form.setFields(cells);
      message.error(others.length > 0 ? others.join('；') : '資料有誤，請檢查標示的欄位');
      return;
    }
    message.error(errorMessage(err));
  };

  const runAction = (target: DocumentAction) => {
    if (!record) {
      return;
    }
    const mutation = target === 'post' ? postRecord : voidRecord;
    const label = DOCUMENT_ACTION_LABELS[target];
    modal.confirm({
      title: `確定要${label}這張${entity.label}？`,
      content: ACTION_CONFIRM[target],
      okText: label,
      okButtonProps: target === 'void' ? { danger: true } : undefined,
      cancelText: '取消',
      onOk: () =>
        mutation.mutateAsync(record).then(
          () => void message.success(`已${label}`),
          (err: unknown) => {
            // 版本衝突沿用存檔衝突的提示（重新載入）；其他錯誤以易懂的訊息列在表單上方
            if (apiErrorCode(err) === 'CONFLICT') {
              setConflict(true);
            } else {
              setActionError(documentActionError(err, target));
            }
          },
        ),
    });
  };

  const onFinish = () => {
    // 包含 Form.List 內的明細列（onFinish 的參數只有註冊過的欄位，明細的 __id 不在其中）
    const values = form.getFieldsValue(true) as FormValues;
    const lineValues = values[LINES_FORM_KEY];
    if (!record) {
      const lines = buildCreateLines(details, lineValues);
      createRecord.mutate(
        { data: buildCreateData(visible, values), ...(lines ? { lines } : {}) },
        {
          onSuccess: (saved) => {
            markDirty(false);
            message.success('已新增');
            navigate(recordPath(entityKey, saved.id), { replace: true });
          },
          onError: onSaveError,
        },
      );
      return;
    }
    const data = buildUpdateData(visible, values, baseline);
    const lines = buildUpdateLines(details, lineValues, lineBaseline);
    if (Object.keys(data).length === 0 && !lines) {
      message.info('沒有需要儲存的修改');
      return;
    }
    updateRecord.mutate(
      { id: record.id, req: { version: record.version, data, ...(lines ? { lines } : {}) } },
      {
        onSuccess: () => {
          markDirty(false);
          message.success('已儲存');
        },
        onError: onSaveError,
      },
    );
  };

  const title = record ? entity.label : `新增${entity.label}`;

  return (
    <Card
      title={
        <Space>
          {entityIcon(entity.icon)}
          {title}
          {status && <DocumentStatusTag status={status} />}
          {dirty && <Tag color="orange">未存檔</Tag>}
        </Space>
      }
      extra={
        <Space>
          {action && (
            <Tooltip title={dirty ? SAVE_BEFORE_ACTION : undefined}>
              <Button
                type={action === 'post' ? 'primary' : 'default'}
                danger={action === 'void'}
                icon={action === 'post' ? <CheckCircleOutlined /> : <StopOutlined />}
                disabled={dirty}
                loading={postRecord.isPending || voidRecord.isPending}
                onClick={() => runAction(action)}
              >
                {DOCUMENT_ACTION_LABELS[action]}
              </Button>
            </Tooltip>
          )}
          <Button icon={<ArrowLeftOutlined />} onClick={() => navigate(recordListPath(entityKey))}>
            返回列表
          </Button>
        </Space>
      }
    >
      {actionError && (
        <Alert
          showIcon
          closable
          type="error"
          style={{ marginBottom: 16 }}
          message={actionError.title}
          description={
            actionError.items.length > 0 ? (
              <ul style={{ margin: 0, paddingInlineStart: 20 }}>
                {actionError.items.map((item) => (
                  <li key={item}>{item}</li>
                ))}
              </ul>
            ) : undefined
          }
          action={
            actionError.reload ? (
              <Button icon={<ReloadOutlined />} onClick={onReload}>
                重新載入
              </Button>
            ) : undefined
          }
          onClose={() => setActionError(undefined)}
        />
      )}
      {isDocument && record && status !== 'draft' && (
        <Typography.Paragraph type="secondary">
          {status === 'void' ? '此單據已作廢，' : '此單據已過帳，'}不能修改。
          {record.document?.posted_at && `過帳時間：${formatTaipei(record.document.posted_at)}`}
          {record.document?.voided_at && `；作廢時間：${formatTaipei(record.document.voided_at)}`}
        </Typography.Paragraph>
      )}
      {conflict && (
        <Alert
          showIcon
          type="warning"
          style={{ marginBottom: 16 }}
          message={RECORD_CONFLICT_MESSAGE}
          description="重新載入會取得最新的資料，並放棄目前未存檔的修改。"
          action={
            <Button icon={<ReloadOutlined />} onClick={onReload}>
              重新載入
            </Button>
          }
        />
      )}
      {displayed.length === 0 && details.length === 0 ? (
        <Empty description="此資料類別沒有可以輸入的欄位" />
      ) : (
        <Form<FormValues>
          form={form}
          layout="vertical"
          initialValues={initialValues}
          disabled={!editable}
          onValuesChange={onValuesChange}
          onFinish={onFinish}
        >
          <div style={{ maxWidth: 720 }}>
            {showsRecordNo(entity, record) && (
              <Form.Item label="單號">
                <Input
                  aria-label="單號"
                  readOnly
                  disabled
                  style={{ maxWidth: 320 }}
                  value={record?.record_no ?? ''}
                  placeholder={RECORD_NO_PENDING}
                />
              </Form.Item>
            )}
            {displayed.map((field) => {
              if (field.data_type === 'formula') {
                const cell = formulaCell(preview, null, 0, field.field_key, {
                  data: record?.data,
                  errors: record?.formula_errors,
                });
                return (
                  <Form.Item
                    key={field.field_key}
                    label={field.label}
                    extra={field.help_text ?? undefined}
                  >
                    <FormulaValue
                      field={field}
                      value={cell.value}
                      error={cell.error}
                      pending={cell.pending && (dirty || !record)}
                    />
                  </Form.Item>
                );
              }
              return (
                <Form.Item
                  key={field.field_key}
                  name={field.field_key}
                  label={field.label}
                  extra={field.help_text ?? undefined}
                  required={field.is_required && !field.is_readonly}
                  rules={[
                    {
                      validator: (_, value: unknown) => {
                        const error = validateFieldValue(field, value);
                        return error ? Promise.reject(new Error(error)) : Promise.resolve();
                      },
                    },
                  ]}
                >
                  {/* 傳 false 會蓋過 Form 的 disabled（唯讀者），只在唯讀欄位時指定 */}
                  <RecordFieldInput
                    field={field}
                    disabled={field.is_readonly || undefined}
                    relation={record?.relations?.[field.field_key]}
                    attachment={record?.attachments?.[field.field_key]}
                  />
                </Form.Item>
              );
            })}
          </div>
          {details.map((table) => (
            <LinesTable
              key={table.entity.key}
              table={table}
              canWrite={editable}
              refs={refs}
              onRowsChange={onValuesChange}
              showPending={dirty || !record}
              formulaCell={(index, lineId, fieldKey) => {
                const saved = lineId ? savedLineById.get(lineId) : undefined;
                return formulaCell(preview, table.entity.key, index, fieldKey, {
                  data: saved?.data,
                  errors: saved?.formula_errors,
                });
              }}
            />
          ))}
          {editable && (
            <Form.Item>
              <Button
                type="primary"
                htmlType="submit"
                icon={<SaveOutlined />}
                loading={saving}
                disabled={record !== undefined && !dirty}
              >
                儲存
              </Button>
            </Form.Item>
          )}
        </Form>
      )}
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
    </Card>
  );
}

/**
 * 新增資料（`/entities/:key/new`，需要 `record.write`）與檢視、編輯資料
 * （`/entities/:key/:id`，需要 `record.read`；沒有 `record.write` 時只能檢視）。
 *
 * 表單依欄位定義產生：有效且設定為顯示的欄位，依排序；驗證規則與伺服器相同。
 */
export function RecordFormPage() {
  const { key = '', id } = useParams();
  const entity = useEntity(key);
  const entities = useEntities();
  const record = useRecord(key, id);
  const detailEntities = useMemo(
    () =>
      entity.data && entities.data
        ? detailEntitiesOf(entities.data, entity.data.entity)
        : undefined,
    [entity.data, entities.data],
  );
  const detailDefs = useEntityDetails(detailEntities);
  const details = useMemo<DetailTable[] | undefined>(
    () =>
      detailDefs.data?.map((d) => ({
        entity: d.entity,
        fields: formFields(d.fields),
        formulas: formulaFields(d.fields),
      })),
    [detailDefs.data],
  );

  const error = entity.error ?? entities.error ?? detailDefs.error ?? record.error;
  if (error) {
    return (
      <Card title="資料">
        <Alert showIcon type="error" message={errorMessage(error)} />
      </Card>
    );
  }
  if (!entity.data || !details || !detailDefs.data || (id !== undefined && !record.data)) {
    return <Card title="資料" loading />;
  }
  const detail = entity.data;
  if (!detail.entity.is_active) {
    return (
      <Card title={detail.entity.label}>
        <Empty description="此資料類別已停用" />
      </Card>
    );
  }
  return (
    <RecordForm
      // 資料重新載入或存檔後，以新的資料重設表單
      key={`${key}:${id ?? 'new'}:${record.dataUpdatedAt}`}
      entity={detail.entity}
      fields={detail.fields}
      details={details}
      detailDefs={detailDefs.data}
      record={record.data}
      onReload={() => void record.refetch()}
    />
  );
}

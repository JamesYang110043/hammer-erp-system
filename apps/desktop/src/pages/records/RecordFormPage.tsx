import { ArrowLeftOutlined, ReloadOutlined, SaveOutlined } from '@ant-design/icons';
import { Alert, App, Button, Card, Empty, Form, Modal, Space, Tag } from 'antd';
import { useEffect, useMemo, useRef, useState } from 'react';
import { useBlocker, useNavigate, useParams } from 'react-router';

import { apiErrorCode, errorMessage } from '../../api/client';
import { useCreateRecord, useRecord, useUpdateRecord } from '../../api/records';
import { useEntity } from '../../api/schema';
import { usePermission } from '../../auth/permission';
import { PERMISSIONS } from '../../bindings';
import type { EntityDefinition, FieldDefinition, Record as DataRecord } from '../../bindings';
import { recordListPath, recordPath } from '../../modules';
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
import { entityIcon } from '../schema/entityIcons';
import { RecordFieldInput } from './RecordFieldInput';

/** 樂觀鎖定衝突（409）的提示（docs/architecture.md §9.4）。 */
export const RECORD_CONFLICT_MESSAGE = '資料已被其他人修改';

/**
 * 新增或編輯一筆資料的表單。`record` 為 `undefined` 時是新增。
 *
 * 父元件在資料重新載入（存檔成功、衝突後重新載入）時以 `key` 重設本元件。
 */
function RecordForm({
  entity,
  fields,
  record,
  onReload,
}: {
  entity: EntityDefinition;
  fields: FieldDefinition[];
  record?: DataRecord;
  onReload: () => void;
}) {
  const entityKey = entity.key;
  const navigate = useNavigate();
  const { message } = App.useApp();
  const [form] = Form.useForm<RecordFormValues>();
  const canWrite = usePermission(PERMISSIONS.RECORD_WRITE);
  const createRecord = useCreateRecord(entityKey);
  const updateRecord = useUpdateRecord(entityKey);
  const saving = createRecord.isPending || updateRecord.isPending;

  const visible = useMemo(() => formFields(fields), [fields]);
  const initialValues = useMemo(() => initialFormValues(visible, record?.data), [visible, record]);
  const baseline = useMemo(() => baselineData(visible, record?.data), [visible, record]);

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

  const onValuesChange = () =>
    markDirty(changedFieldKeys(visible, form.getFieldsValue(true), baseline).length > 0);

  const onSaveError = (err: unknown) => {
    if (apiErrorCode(err) === 'CONFLICT') {
      setConflict(true);
      return;
    }
    const fieldErrors = recordFieldErrors(err);
    if (fieldErrors && fieldErrors.length > 0) {
      const shown = new Set(visible.map((f) => f.field_key));
      form.setFields(
        fieldErrors
          .filter((e) => shown.has(e.field_key))
          .map((e) => ({ name: e.field_key, errors: [e.message] })),
      );
      const others = fieldErrors.filter((e) => !shown.has(e.field_key));
      message.error(
        others.length > 0
          ? others.map((e) => `${e.field_key}：${e.message}`).join('；')
          : '資料有誤，請檢查標示的欄位',
      );
      return;
    }
    message.error(errorMessage(err));
  };

  const onFinish = (values: RecordFormValues) => {
    if (!record) {
      createRecord.mutate(
        { data: buildCreateData(visible, values) },
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
    if (Object.keys(data).length === 0) {
      message.info('沒有需要儲存的修改');
      return;
    }
    updateRecord.mutate(
      { id: record.id, req: { version: record.version, data } },
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
          {dirty && <Tag color="orange">未存檔</Tag>}
        </Space>
      }
      extra={
        <Button icon={<ArrowLeftOutlined />} onClick={() => navigate(recordListPath(entityKey))}>
          返回列表
        </Button>
      }
    >
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
      {visible.length === 0 ? (
        <Empty description="此資料類別沒有可以輸入的欄位" />
      ) : (
        <Form<RecordFormValues>
          form={form}
          layout="vertical"
          style={{ maxWidth: 720 }}
          initialValues={initialValues}
          disabled={!canWrite}
          onValuesChange={onValuesChange}
          onFinish={onFinish}
        >
          {visible.map((field) => (
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
              <RecordFieldInput field={field} disabled={field.is_readonly || undefined} />
            </Form.Item>
          ))}
          {canWrite && (
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
  const record = useRecord(key, id);

  const error = entity.error ?? record.error;
  if (error) {
    return (
      <Card title="資料">
        <Alert showIcon type="error" message={errorMessage(error)} />
      </Card>
    );
  }
  if (!entity.data || (id !== undefined && !record.data)) {
    return <Card title="資料" loading />;
  }
  const detail = entity.data;
  if (detail.entity.storage !== 'custom_records' || !detail.entity.is_active) {
    return (
      <Card title={detail.entity.label}>
        <Empty description="此資料類別尚未開放輸入" />
      </Card>
    );
  }
  return (
    <RecordForm
      // 資料重新載入或存檔後，以新的資料重設表單
      key={`${key}:${id ?? 'new'}:${record.dataUpdatedAt}`}
      entity={detail.entity}
      fields={detail.fields}
      record={record.data}
      onReload={() => void record.refetch()}
    />
  );
}

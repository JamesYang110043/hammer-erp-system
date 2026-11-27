import { ImportOutlined } from '@ant-design/icons';
import { App, Button, Modal, Space, Table, Typography, theme } from 'antd';
import type { TableColumnsType } from 'antd';
import { useState } from 'react';

import { CommandError, errorMessage, unwrap } from '../../api/client';
import { useCustomerProductNamesBatch } from '../../api/sales';
import { commands } from '../../bindings';
import type {
  CustomerProductNameBatchResult_Serialize as CustomerProductNameBatchResult,
  CustomerProductNameImport,
  ImportedCustomerProductName,
} from '../../bindings';
import {
  IMPORT_ACTION_LABELS,
  batchRowsOf,
  importBlockedReason,
  importSuccessText,
  importSummaryText,
  rejectedBatchResult,
} from '../../records/customerProductNameImport';

/** 預覽中的狀態：選擇的檔案與伺服器的檢查結果。 */
interface Preview {
  file: CustomerProductNameImport;
  result: CustomerProductNameBatchResult;
}

/** 預覽表格的一列：Excel 的內容與伺服器的結果。 */
type PreviewRow = ImportedCustomerProductName & {
  index: number;
  result: CustomerProductNameBatchResult['rows'][number] | undefined;
};

/**
 * 客戶畫面的「匯入客戶品名」（docs/architecture.md §8.7「Excel 匯入客戶品名」；只有 `record.write`
 * 顯示，由呼叫端判斷）。
 *
 * 選擇 `.xlsx`（Tauri 端解析）→ 以 `dry_run` 呼叫批次寫入 API 並顯示預覽（每列的動作與錯誤）→ 沒有錯誤
 * 時可以匯入。有錯誤時「匯入」停用，修正 Excel 後重新選擇檔案。
 */
export function CustomerProductNamesImport() {
  const { message } = App.useApp();
  const { token } = theme.useToken();
  const batch = useCustomerProductNamesBatch();
  const [picking, setPicking] = useState(false);
  const [preview, setPreview] = useState<Preview | undefined>();

  const pick = async () => {
    setPicking(true);
    try {
      const file = await unwrap(commands.pickCustomerProductNamesXlsx());
      if (!file) {
        return;
      }
      const result = await batch.mutateAsync({ dry_run: true, rows: batchRowsOf(file.rows) });
      setPreview({ file, result });
    } catch (err) {
      message.error(errorMessage(err));
    } finally {
      setPicking(false);
    }
  };

  const runImport = async () => {
    if (!preview) {
      return;
    }
    try {
      const result = await batch.mutateAsync({
        dry_run: false,
        rows: batchRowsOf(preview.file.rows),
      });
      setPreview(undefined);
      message.success(importSuccessText(result));
    } catch (err) {
      const api = err instanceof CommandError && err.error.kind === 'api' ? err.error : undefined;
      const rejected = rejectedBatchResult(api);
      if (rejected) {
        // 預覽之後資料被別人改了：以最新的結果更新預覽
        setPreview({ ...preview, result: rejected });
        message.error(errorMessage(err));
        return;
      }
      message.error(errorMessage(err));
    }
  };

  const rows: PreviewRow[] = (preview?.file.rows ?? []).map((r, index) => ({
    ...r,
    index,
    result: preview?.result.rows[index],
  }));
  const blocked = preview ? importBlockedReason(preview.result) : undefined;
  const columns: TableColumnsType<PreviewRow> = [
    { key: 'row_no', title: '列號', dataIndex: 'row_no', width: 64, align: 'right' },
    { key: 'customer_code', title: '客戶編號', dataIndex: 'customer_code' },
    { key: 'product_code', title: '料號', dataIndex: 'product_code' },
    { key: 'name', title: '客戶品名', dataIndex: 'name' },
    {
      key: 'action',
      title: '動作',
      width: 72,
      render: (_, row) =>
        row.result?.action ? IMPORT_ACTION_LABELS[row.result.action] : <span>—</span>,
    },
    {
      key: 'errors',
      title: '錯誤',
      render: (_, row) =>
        row.result?.errors.length ? (
          <Typography.Text type="danger">
            {row.result.errors.map((e) => e.message).join('；')}
          </Typography.Text>
        ) : null,
    },
  ];

  return (
    <>
      <Button icon={<ImportOutlined />} onClick={pick} loading={picking}>
        匯入客戶品名
      </Button>
      <Modal
        open={preview !== undefined}
        title={`匯入客戶品名：${preview?.file.file_name ?? ''}`}
        width={960}
        onCancel={() => setPreview(undefined)}
        maskClosable={false}
        footer={
          <Space wrap style={{ justifyContent: 'flex-end' }}>
            {blocked && <Typography.Text type="secondary">{blocked}</Typography.Text>}
            <Button onClick={pick} loading={picking}>
              重新選擇檔案
            </Button>
            <Button onClick={() => setPreview(undefined)}>取消</Button>
            <Button
              type="primary"
              disabled={blocked !== undefined}
              loading={batch.isPending && !picking}
              onClick={runImport}
            >
              匯入
            </Button>
          </Space>
        }
      >
        {preview && (
          <>
            <Typography.Paragraph aria-label="匯入筆數">
              {importSummaryText(preview.result)}
            </Typography.Paragraph>
            <Table<PreviewRow>
              rowKey="index"
              size="small"
              columns={columns}
              dataSource={rows}
              pagination={rows.length > 100 ? { pageSize: 100, showSizeChanger: false } : false}
              scroll={{ x: 'max-content', y: 420 }}
              onRow={(row) =>
                row.result?.errors.length
                  ? { style: { background: token.colorErrorBg }, 'aria-invalid': true }
                  : {}
              }
            />
          </>
        )}
      </Modal>
    </>
  );
}

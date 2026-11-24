import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Tag } from 'antd';
import { describe, expect, it, vi } from 'vitest';

import { fieldDef } from '../test/schemaFixtures';
import { FieldValue } from '../pages/records/FieldValue';
import { EmptyState } from './EmptyState';
import { NumberText } from './NumberText';
import { Page, PageHeader } from './PageHeader';

describe('PageHeader', () => {
  it('renders_title_tags_description_and_actions', async () => {
    const onBack = vi.fn();
    render(
      <PageHeader
        title="盤點調整單"
        tags={<Tag>草稿</Tag>}
        description="說明文字"
        actions={<button type="button">新增</button>}
        onBack={onBack}
        backLabel="返回列表"
      />,
    );
    expect(screen.getByRole('heading', { level: 4, name: '盤點調整單' })).toBeInTheDocument();
    expect(screen.getByText('草稿')).toBeInTheDocument();
    expect(screen.getByText('說明文字')).toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: '新增' }).closest('.page-header-actions'),
    ).not.toBeNull();
    await userEvent.click(screen.getByRole('button', { name: '返回列表' }));
    expect(onBack).toHaveBeenCalled();
  });

  it('page_puts_content_in_a_card_below_the_header', () => {
    render(<Page title="幣別">內容</Page>);
    const header = screen.getByTestId('page-header');
    const card = screen.getByText('內容').closest('.ant-card')!;
    expect(header.compareDocumentPosition(card) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(card.querySelector('.ant-card-head')).toBeNull();
  });
});

describe('NumberText', () => {
  it('negative_numbers_use_the_error_color', () => {
    render(
      <>
        <NumberText>-1,234.50</NumberText>
        <NumberText>1,234.50</NumberText>
      </>,
    );
    expect(screen.getByText('-1,234.50')).toHaveClass('ant-typography-danger');
    expect(screen.getByText('1,234.50')).not.toHaveClass('ant-typography-danger');
    expect(screen.getByText('1,234.50')).toHaveClass('num');
  });

  it('field_value_formats_money_and_marks_negative', () => {
    render(
      <FieldValue field={fieldDef({ data_type: 'money', decimal_places: 2 })} value="-1234.5" />,
    );
    expect(screen.getByText('-1,234.50')).toHaveClass('ant-typography-danger');
  });
});

describe('EmptyState', () => {
  it('shows_preset_title_and_guidance', () => {
    render(<EmptyState kind="forbidden" description="請洽管理者" />);
    const status = screen.getByRole('status');
    expect(status).toHaveTextContent('沒有權限');
    expect(status).toHaveTextContent('請洽管理者');
  });
});

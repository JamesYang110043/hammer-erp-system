import { screen, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router';
import { describe, expect, it } from 'vitest';

import { commandError, mockCommands } from '../../test/ipc';
import { renderWithProviders } from '../../test/render';
import { NO_CUSTOMER_PRODUCT_NAMES, ProductCustomerNames } from './ProductCustomerNames';

const ROWS = [
  {
    id: 'n1',
    customer_id: 'c1',
    customer_title: 'C001 大華實業',
    customer_is_active: true,
    name: 'X 型扳手',
    notes: '圖面 W-10',
  },
  {
    id: 'n2',
    customer_id: 'c3',
    customer_title: 'C003 東昇貿易',
    customer_is_active: false,
    name: '東昇的扳手',
    notes: null,
  },
];

function renderBlock() {
  return renderWithProviders(
    <MemoryRouter>
      <ProductCustomerNames productId="p1" />
    </MemoryRouter>,
  );
}

describe('ProductCustomerNames (§8.7 商品畫面的客戶品名)', () => {
  it('lists customers in API order with links to the customer split view', async () => {
    const invoke = mockCommands({ product_customer_names: () => ROWS });
    renderBlock();
    const block = await screen.findByRole('region', { name: '客戶品名' });
    const link = await within(block).findByRole('link', { name: 'C001 大華實業' });
    expect(link.getAttribute('href')).toBe('/entities/customer?selected=c1');
    const rows = within(block).getAllByRole('row').slice(1);
    expect(rows.map((r) => r.textContent)).toEqual([
      'C001 大華實業X 型扳手圖面 W-10',
      'C003 東昇貿易停用東昇的扳手—',
    ]);
    expect(invoke).toHaveBeenCalledWith('product_customer_names', { productId: 'p1' });
  });

  it('shows the empty state', async () => {
    mockCommands({ product_customer_names: () => [] });
    renderBlock();
    expect(await screen.findByText(NO_CUSTOMER_PRODUCT_NAMES)).toBeTruthy();
  });

  it('shows an error when the request fails', async () => {
    mockCommands({
      product_customer_names: () => commandError({ kind: 'network', message: '無法連線到伺服器' }),
    });
    renderBlock();
    expect(await screen.findByText(/無法讀取客戶品名/)).toBeTruthy();
  });
});

import { EmptyState } from '../components/EmptyState';
import { Page } from '../components/PageHeader';

/** 尚未開發的模組。 */
export function PlaceholderPage({ title }: { title: string }) {
  return (
    <Page title={title}>
      <EmptyState kind="comingSoon" description="將於之後的版本提供" />
    </Page>
  );
}

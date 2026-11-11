import { Blend } from 'lucide-react';
import { useI18n } from '../i18n';

/** Header metadata only: an Alpha channel does not prove transparent pixels or masks. */
export function AlphaSourceStatus({ detected, readOnly = false }: { detected?: boolean | null; readOnly?: boolean }) {
  const { t } = useI18n();
  if (detected !== true) return null;
  const description = t('alpha.imagesTitle');
  return <span className={`alpha-source-status source-attribute-tag${readOnly ? ' readonly' : ''}`} role="status" title={description} aria-label={description} tabIndex={0}>
    <Blend size={13} aria-hidden="true" /><span>{t('source.alphaTag')}</span>
  </span>;
}

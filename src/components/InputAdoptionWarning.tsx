import { CircleAlert } from 'lucide-react';
import { localizePipelineMessage, useI18n } from '../i18n';
import { taskBasename } from '../app/taskPresentation';
import type { SavedInputs } from '../types/inputs';

interface Props {
  inputs?: SavedInputs | null;
  warning?: string | null;
}

const isAdoptionWarning = (value: string) => value.startsWith('部分素材未能参与生成：')
  || value.startsWith('Some inputs could not be included:')
  || /补拍画面已加入 [\d,]+\/[\d,]+ 张/.test(value)
  || /Added [\d,]+\/[\d,]+ reshoot images\./.test(value);

function inputLabel(inputs: SavedInputs, id: string): string {
  if (inputs.request.mode === 'materials') {
    const item = inputs.request.items.find(candidate => candidate.id === id);
    if (item) {
      const path = item.source.kind === 'images' ? item.source.paths[0] : item.source.path;
      if (path) return taskBasename(path);
    }
  }
  const source = inputs.sources.find(candidate => candidate.id === id);
  return source?.names[0] ?? id;
}

/** Only durable per-source counts or explicit persisted warnings are used. */
export function InputAdoptionWarning({ inputs, warning }: Props) {
  const { locale, t } = useI18n();
  const unused = inputs
    ? [...new Set([
      ...inputs.unused,
      ...(inputs.sourceAdoption ?? []).filter(source => !source.adopted).map(source => source.sourceId),
    ])]
    : [];
  const messages = inputs && unused.length > 0
    ? [t('inputs.unused', {
      names: unused.map(id => inputLabel(inputs, id)).join(locale === 'zh-CN' ? '、' : ', '),
    })] : [];
  const savedMessages = (warning ?? '').split(/\r?\n/).filter(isAdoptionWarning);
  for (const saved of savedMessages) {
    // The omitted-source message above already uses stable saved source labels.
    if (unused.length && /^(部分素材未能参与生成：|Some inputs could not be included:)/.test(saved)) continue;
    messages.push(localizePipelineMessage(locale, saved));
  }
  if (inputs?.sources.some(source => source.id === '__source') && !savedMessages.some(message => /补拍画面已加入|reshoot images\./.test(message))) {
    const added = (inputs.sourceAdoption ?? []).filter(source => source.sourceId !== '__source'
      && Number.isFinite(source.preparedFrames) && source.preparedFrames > 0
      && Number.isFinite(source.registeredFrames) && source.registeredFrames >= 0);
    const prepared = added.reduce((sum, source) => sum + source.preparedFrames, 0);
    const registered = added.reduce((sum, source) => sum + source.registeredFrames, 0);
    const lowMessage = (current: number, total: number) => t('reshoot.lowRegistration', { registered: current, prepared: total });
    if (registered > 0 && prepared > 0 && registered / prepared < 0.8) messages.push(lowMessage(registered, prepared));
    else for (const source of added) {
      if (source.registeredFrames > 0 && source.registeredFrames / source.preparedFrames < 0.8) {
        messages.push(`${inputLabel(inputs, source.sourceId)}${locale === 'zh-CN' ? '：' : ': '}${lowMessage(source.registeredFrames, source.preparedFrames)}`);
      }
    }
  }
  if (!messages.length) return null;
  return <>{[...new Set(messages)].map(message => <p key={message} className="project-quality-warning input-adoption-warning" role="status"><CircleAlert size={13} />{message}</p>)}</>;
}

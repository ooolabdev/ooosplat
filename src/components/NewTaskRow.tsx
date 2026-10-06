import type { PointerEventHandler } from 'react';
import { GripVertical, Trash2 } from 'lucide-react';
import { useI18n } from '../i18n';
import { displayPath } from '../lib/displayPath';
import { CompactError } from './CompactError';

interface Props {
  draftId?: string; taskId?: string;
  title: string; path: string | null; status: string; statusLabel: string;
  selected: boolean; canReorder: boolean; error?: string | null; agentCreated?: boolean;
  insertion?: 'before' | 'after' | null; dragging?: boolean;
  onSelect: () => void; onDelete?: () => void;
  onPointerDown?: PointerEventHandler<HTMLElement>; onPointerMove?: PointerEventHandler<HTMLElement>; onPointerEnd?: PointerEventHandler<HTMLElement>;
  onKeyboardMove?: (offset: -1 | 1) => void;
}

/** Keep the original draft layout when a task is submitted; only drafts can reorder. */
export function NewTaskRow({ draftId, taskId, title, path, status, statusLabel, selected, canReorder, error, agentCreated, insertion, dragging, onSelect, onDelete, onPointerDown, onPointerMove, onPointerEnd, onKeyboardMove }: Props) {
  const { locale, t } = useI18n();
  const visiblePath = path ? displayPath(path) : null;
  return <article data-draft-id={draftId} data-task-id={taskId} className={`project-row draft-row${selected ? ' selected' : ''}${canReorder ? '' : ' drag-disabled'}${dragging ? ' dragging' : ''}${insertion ? ` drop-${insertion}` : ''}`} tabIndex={0} role="button" aria-pressed={selected} aria-keyshortcuts={canReorder ? 'Alt+ArrowUp Alt+ArrowDown' : undefined} onClick={onSelect}
    onPointerDown={canReorder ? onPointerDown : undefined} onPointerMove={canReorder ? onPointerMove : undefined} onPointerUp={canReorder ? onPointerEnd : undefined} onPointerCancel={canReorder ? onPointerEnd : undefined}
    onKeyDown={event => {
      if (event.target !== event.currentTarget) return;
      if (event.altKey && canReorder && onKeyboardMove && (event.key === 'ArrowUp' || event.key === 'ArrowDown')) { event.preventDefault(); onKeyboardMove(event.key === 'ArrowUp' ? -1 : 1); }
      else if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); onSelect(); }
    }}>
    <span className="draft-drag-indicator" aria-hidden="true"><GripVertical size={15} /></span>
    <div className="project-title-line"><span className={`project-status ${status}`} /><strong>{title}</strong><span className="status-copy">{statusLabel}</span></div>
    <p className="project-path" title={visiblePath ?? undefined}>{visiblePath ?? t('workspace.awaitingInput')}</p>
    {agentCreated && <p className="task-origin">{locale === 'zh-CN' ? '由 AI Agent 创建' : 'Created by AI Agent'}</p>}
    {error && <p className="project-failure"><CompactError message={error} /></p>}
    <div className="project-actions"><button className="danger-link" type="button" disabled={!canReorder || !onDelete} onClick={event => { event.stopPropagation(); onDelete?.(); }}><Trash2 size={14} />{t('project.delete')}</button></div>
  </article>;
}

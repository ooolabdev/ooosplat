import { useEffect, useMemo, useState } from 'react';
import { useI18n } from '../i18n';
import { getProjectTaskDetail } from '../lib/backend';
import { mergeActivityEvents } from '../lib/taskActivity';
import { eventLogLines, legacyDisplayLines } from '../lib/taskLogDisplay';
import type { ProjectSummary, ProjectTaskDetail } from '../types/pipeline';
import { TaskProgress } from './TaskProgress';

export function HistoricalTaskProgress({ project, detail }: { project: ProjectSummary; detail: ProjectTaskDetail }) {
  const { locale } = useI18n();
  type LogFields = Pick<ProjectTaskDetail, 'activityEvents' | 'activityJournalPersisted' | 'logsUnavailable' | 'logs'>;
  // Configuration/status updates are not log reads. A genuinely new log read
  // supplies new arrays, even when it retained the same lines after an I/O
  // error; it must not inherit a previous successful repair's error flag.
  const snapshot = useMemo<LogFields>(() => ({ activityEvents: detail.activityEvents,
    activityJournalPersisted: detail.activityJournalPersisted,
    logsUnavailable: detail.logsUnavailable, logs: detail.logs,
  }), [detail.activityEvents, detail.activityJournalPersisted, detail.logsUnavailable, detail.logs]);
  const [recovered, setRecovered] = useState<{ projectId: string; original: LogFields; logs: LogFields } | null>(null);
  const logDetail = recovered?.projectId === project.id && recovered.original === snapshot ? recovered.logs : snapshot;
  useEffect(() => {
    if (snapshot.logsUnavailable !== true) return;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    const retry = async () => {
      try {
        const next = await getProjectTaskDetail(project.id);
        if (disposed) return;
        // A log repair must not adopt another snapshot's task status, phase,
        // configuration or training counters into this historical view.
        setRecovered({ projectId: project.id, original: snapshot, logs: {
          activityEvents: next.activityEvents, activityJournalPersisted: next.activityJournalPersisted,
          logsUnavailable: next.logsUnavailable, logs: next.logs,
        } });
        if (next.logsUnavailable !== true) return;
      } catch { if (disposed) return; }
      timer = setTimeout(() => void retry(), 15000);
    };
    timer = setTimeout(() => void retry(), 15000);
    return () => { disposed = true; clearTimeout(timer); };
  }, [project.id, snapshot]);
  const logs = useMemo(() => eventLogLines(mergeActivityEvents([], logDetail.activityEvents ?? []), locale,
    `project:${project.id}`), [logDetail.activityEvents, locale, project.id]);
  const rawLogs = useMemo(() => logDetail.activityEvents != null ? [] : legacyDisplayLines(logDetail.logs.map((line, index) => ({
    key: `legacy:${line.source}:${index}`, source: line.source, text: line.message,
  })), logs), [logDetail.activityEvents, logDetail.logs, logs]);
  return <TaskProgress importedReconstruction={detail.inputs?.importedReconstruction} progressKey={`project:${project.id}`}
    historical status={project.status} stage={detail.stage} progress={detail.progress} logs={logs} rawLogs={rawLogs}
    logsUnavailable={logDetail.logsUnavailable === true}
    logsTruncated={logDetail.activityJournalPersisted === true && logs.length >= 500} />;
}

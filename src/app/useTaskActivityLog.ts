import { useEffect, useMemo, useRef, useState } from 'react';
import { readSharedTaskLogs, type TaskLogPage } from '../lib/backend';
import { ACTIVITY_BYTES, ACTIVITY_LIMIT, mergeActivityEvents } from '../lib/taskActivity';
import type { Locale } from '../i18n';
import { eventLogLines, legacyDisplayLines } from '../lib/taskLogDisplay';
import type { PipelineEvent } from '../types/pipeline';
import type { TaskLogLine } from '../components/TaskProgress';

interface History {
  key: string;
  events: PipelineEvent[];
  raw: TaskLogLine[];
  structured: boolean;
  persisted: boolean;
  unavailable: boolean;
  truncated: boolean;
  cursorReset: boolean;
}
interface Session {
  key: string; cursor?: string; number: number; pending?: Promise<TaskLogPage>;
  cursorResetAttempted?: boolean;
  replayedSources?: Set<string>;
}
const EMPTY_EVENTS: PipelineEvent[] = [];
const EMPTY_RAW: TaskLogLine[] = [];
const emptyHistory = (key: string): History => ({ key, events: [], raw: [], structured: false, persisted: false, unavailable: false, truncated: false, cursorReset: false });

function errorCode(error: unknown): string | undefined {
  if (typeof error === 'string') {
    try { return errorCode(JSON.parse(error)); } catch { return undefined; }
  }
  return error && typeof error === 'object' && 'code' in error && typeof error.code === 'string' ? error.code : undefined;
}

function boundRaw(lines: TaskLogLine[]): TaskLogLine[] {
  let bytes = 0;
  const retained: TaskLogLine[] = [];
  const seen = new Set<string>();
  for (let index = lines.length - 1; index >= 0 && retained.length < ACTIVITY_LIMIT; index--) {
    const line = lines[index];
    if (seen.has(line.key)) continue;
    seen.add(line.key);
    bytes += line.text.length;
    if (bytes > ACTIVITY_BYTES) break;
    retained.push(line);
  }
  return retained.reverse();
}

/** One cancellable reader for both task views. File history never drives task
 * status, progress, runtime, or the live event watermark.
 */
export function useTaskActivityLog(taskId: string | null | undefined, runId: string | null | undefined,
  active: boolean, liveEvents: PipelineEvent[], locale: Locale) {
  const key = JSON.stringify([taskId ?? null, runId ?? null]);
  const [history, setHistory] = useState<History>(() => emptyHistory(key));
  const sessionRef = useRef<Session>({ key, number: 0 });
  useEffect(() => {
    if (!taskId) return;
    if (sessionRef.current.key !== key) sessionRef.current = { key, number: 0 };
    const session = sessionRef.current;
    setHistory(current => current.key === key ? current : emptyHistory(key));
    let disposed = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const schedule = (delay: number) => { if (!disposed) timer = setTimeout(() => void read(), delay); };
    const read = async () => {
      let more = false;
      try {
        // Drain bounded pages serially, then yield to the browser before the
        // next group. Never issue overlapping reads or wait 15s per page.
        for (let pageNumber = 0; pageNumber < 8; pageNumber++) {
          if (session.pending) await session.pending.catch(() => undefined);
          if (disposed || sessionRef.current !== session) return;
          const previousCursor = session.cursor;
          const pending = readSharedTaskLogs(taskId, runId ?? null, previousCursor);
          session.pending = pending;
          let page: TaskLogPage;
          try { page = await pending; }
          finally { if (session.pending === pending) session.pending = undefined; }
          if (disposed || sessionRef.current !== session) return;
          session.cursor = page.next_cursor ?? undefined;
          session.cursorResetAttempted = false;
          const resetSources = new Set(page.cursor_reset ? page.entries.map(entry => entry.source) : []);
          if (session.replayedSources) {
            for (const entry of page.entries) {
              if (!session.replayedSources.has(entry.source)) resetSources.add(entry.source);
              session.replayedSources.add(entry.source);
            }
          }
          const raw = page.entries.flatMap(entry => entry.text.split(/\r?\n/).filter(Boolean).map(text => ({
            key: `raw:${session.number++}`, source: entry.source, text,
          })));
          setHistory(current => {
            const previous = current.key === key ? current : emptyHistory(key);
            const structured = previous.structured || page.activity_events != null;
            return {
              key, structured,
              events: mergeActivityEvents(previous.events, page.activity_events ?? EMPTY_EVENTS,
                { taskId, runId: runId ?? null }),
              // Raw output has no reliable global sequence. Keep it separate
              // for legacy runs; never parse text into invented activity events.
              raw: structured ? EMPTY_RAW : boundRaw([...previous.raw.filter(line => !resetSources.has(line.source ?? 'system')), ...raw]),
              persisted: page.activity_journal_persisted === true,
              unavailable: false,
              truncated: previous.truncated || page.truncated === true || page.cursor_reset,
              cursorReset: previous.cursorReset || page.cursor_reset,
            };
          });
          more = page.has_more;
          if (more && (session.cursor === previousCursor || session.cursor == null)) throw new Error('Log cursor did not advance');
          if (!more) break;
        }
        if (!more) session.replayedSources = undefined;
        if (more) schedule(0);
        else if (active) schedule(15000);
      } catch (error) {
        if (disposed || sessionRef.current !== session) return;
        // A source/filter mismatch or expired signature permits one cursor
        // rebase. Real I/O failures retry the same position without duplicating
        // a legacy tail; a successful read renews the rebase allowance.
        const code = errorCode(error);
        if ((code === 'CURSOR_MISMATCH' || code === 'CURSOR_INVALID_OR_EXPIRED') && !session.cursorResetAttempted) {
          session.cursorResetAttempted = true;
          session.cursor = undefined;
          session.replayedSources = new Set();
        }
        setHistory(current => ({ ...(current.key === key ? current : emptyHistory(key)), unavailable: true, persisted: false }));
        schedule(15000);
      }
    };
    void read();
    return () => { disposed = true; if (timer != null) clearTimeout(timer); };
  }, [taskId, runId, key, active]);

  const saved = history.key === key ? history : emptyHistory(key);
  const events = useMemo(() => mergeActivityEvents(saved.events, liveEvents,
    taskId ? { taskId, runId: runId ?? null, allowUnbound: true } : undefined), [saved.events, liveEvents, taskId, runId]);
  const logs = useMemo(() => eventLogLines(events, locale, `event:${taskId ?? ''}:${runId ?? ''}`), [events, taskId, runId, locale]);
  const rawLogs = useMemo(() => legacyDisplayLines(saved.raw, logs), [saved.raw, logs]);
  return { events, logs, rawLogs, unavailable: saved.unavailable,
    journalPersisted: saved.persisted, cursorReset: saved.cursorReset,
    truncated: saved.truncated || events.length >= ACTIVITY_LIMIT || rawLogs.length < saved.raw.length };
}

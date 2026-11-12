import { localizePipelineMessage, type Locale } from '../i18n';
import type { PipelineEvent } from '../types/pipeline';
import type { TaskLogLine } from '../components/TaskProgress';
import { ACTIVITY_BYTES, ACTIVITY_LIMIT } from './taskActivity';

export function eventLogLines(events: PipelineEvent[], locale: Locale, prefix: string): TaskLogLine[] {
  return events.map(event => ({
    key: `${prefix}:${event.sequence}`, source: event.engine, timestamp: event.timestamp, level: event.level,
    text: event.kind === 'log' && event.engine !== 'system' ? event.message : localizePipelineMessage(locale, event.message),
  }));
}

/** Bound memory history and the separately labelled legacy source sections
 * together, without inventing a global timeline for raw file output.
 */
export function legacyDisplayLines(raw: TaskLogLine[], events: TaskLogLine[]): TaskLogLine[] {
  let remainingBytes = Math.max(0, ACTIVITY_BYTES - events.reduce((sum, line) => sum + line.text.length, 0));
  let start = raw.length;
  while (start > 0 && raw.length - start < ACTIVITY_LIMIT - events.length) {
    const size = raw[start - 1].text.length;
    if (size > remainingBytes) break;
    remainingBytes -= size;
    start--;
  }
  return raw.slice(start);
}

/** Admission conflicts do not say that the currently executing task failed. */
export function isTaskAdmissionConflict(error: unknown): boolean {
  return typeof error === 'object' && error !== null && 'code' in error
    && (error.code === 'TASK_BUSY' || error.code === 'STALE_RUN_ID');
}

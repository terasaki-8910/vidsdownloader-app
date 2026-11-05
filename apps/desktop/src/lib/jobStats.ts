import type { JobRow } from "../ipc/contract";

export interface JobCounts {
  /** Actively doing work right now (fetching info, transferring, or finishing up). */
  progressing: number;
  /** Not yet being worked on. */
  waiting: number;
  failed: number;
  done: number;
}

/** The four buckets shown in the Downloads title summary (matches the round 3 mock). */
export function summarizeJobs(jobs: JobRow[]): JobCounts {
  const counts: JobCounts = { progressing: 0, waiting: 0, failed: 0, done: 0 };
  for (const job of jobs) {
    switch (job.state) {
      case "Probing":
      case "Downloading":
      case "PostProcessing":
        counts.progressing++;
        break;
      case "Queued":
      case "Ready":
        counts.waiting++;
        break;
      case "Failed":
        counts.failed++;
        break;
      case "Done":
        counts.done++;
        break;
      case "Canceled":
        break; // not shown in the headline summary, same as the mock
    }
  }
  return counts;
}

/** The sidebar nav badge: work that still needs attention or is in flight. */
export function activeJobCount(jobs: JobRow[]): number {
  const { progressing, waiting } = summarizeJobs(jobs);
  return progressing + waiting;
}

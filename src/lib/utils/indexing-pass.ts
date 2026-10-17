import type { IndexingPass } from "$lib/types";

/**
 * A pass duration as Settings shows it: seconds with one decimal under a minute, minutes and
 * seconds from a minute up. The choice is made on the rounded value, so 59,960 ms reads
 * "1 min 0 s" rather than "60.0 s".
 */
export function formatIndexingTime(ms: number): string {
  const tenths = Math.round(ms / 100);
  if (tenths < 600) return `${(tenths / 10).toFixed(1)} s`;
  const seconds = Math.round(ms / 1000);
  return `${Math.floor(seconds / 60)} min ${seconds % 60} s`;
}

/** The pass in one line: its notes, chunks and total time. */
export function indexingPassSummary(pass: IndexingPass): string {
  return `${count(pass.notes, "note")}, ${count(pass.chunks, "chunk")} in ${formatIndexingTime(pass.totalMs)}`;
}

/** The four stage times, labelled and in the order Settings lists them. */
export function indexingPassStages(pass: IndexingPass): { label: string; time: string }[] {
  return [
    { label: "File reads", time: formatIndexingTime(pass.readMs) },
    { label: "Chunking", time: formatIndexingTime(pass.chunkMs) },
    { label: "Ollama", time: formatIndexingTime(pass.embedMs) },
    { label: "Database", time: formatIndexingTime(pass.databaseMs) },
  ];
}

function count(value: number, noun: string): string {
  return `${value} ${noun}${value === 1 ? "" : "s"}`;
}

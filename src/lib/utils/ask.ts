import type MarkdownIt from 'markdown-it';
import type { AskPlan } from '$lib/types';

/** How many answers Ask keeps in memory. Older ones are dropped, never stored. */
export const KEPT_ANSWERS = 3;

export type AskStatus =
  | 'searching'
  | 'reading'
  | 'thinking'
  | 'answering'
  | 'done'
  | 'no-match'
  | 'stopped'
  | 'error';

export interface AskAnswer {
  /** The request id its stream events carry. */
  id: string;
  /** The vault it was asked in; its citations point there. */
  vault: string;
  question: string;
  status: AskStatus;
  plan: AskPlan | null;
  text: string;
  error: string | null;
  /** When the current stage began, for the elapsed-seconds counter. */
  stageStartedAt: number;
}

/** Whether the answer can still change. */
export function isRunning(answer: AskAnswer): boolean {
  return ['searching', 'reading', 'thinking', 'answering'].includes(answer.status);
}

/**
 * Adds `next` as the newest answer. Answers from another vault go first, then the oldest
 * past `KEPT_ANSWERS`; `dropped` lists them so a running one can be cancelled.
 */
export function keepLatest(
  answers: AskAnswer[],
  next: AskAnswer,
): { kept: AskAnswer[]; dropped: AskAnswer[] } {
  const all = [next, ...answers];
  const kept = all.filter((answer) => answer.vault === next.vault).slice(0, KEPT_ANSWERS);
  return { kept, dropped: all.filter((answer) => !kept.includes(answer)) };
}

/**
 * Splits answers by whether they belong to `vault`. A vault switch drops the rest, since
 * their citations point into the vault that was left; a running one must be stopped.
 */
export function forVault(
  answers: AskAnswer[],
  vault: string | null | undefined,
): { kept: AskAnswer[]; dropped: AskAnswer[] } {
  const kept = answers.filter((answer) => Boolean(vault) && answer.vault === vault);
  return { kept, dropped: answers.filter((answer) => !kept.includes(answer)) };
}

/**
 * The answer shown expanded: the one the user picked, unless a newer question has arrived
 * since, in which case the newest, so its progress, errors, and Stop stay visible.
 */
export function expandedAnswer(
  answers: AskAnswer[],
  picked: { id: string; newest: string } | null,
): string | undefined {
  const newest = answers[0]?.id;
  if (picked && picked.newest === newest && answers.some((answer) => answer.id === picked.id)) {
    return picked.id;
  }
  return newest;
}

/** The progress line shown while an answer is on its way. */
export function stageLabel(answer: AskAnswer, now: number): string {
  const seconds = Math.max(0, Math.floor((now - answer.stageStartedAt) / 1000));
  const read = answer.plan?.sources.length ?? 0;
  switch (answer.status) {
    case 'searching':
      return 'Searching notes…';
    case 'reading':
      return `Reading ${read} ${read === 1 ? 'note' : 'notes'}… ${seconds}s`;
    case 'thinking':
      return `Thinking… ${seconds}s`;
    default:
      return '';
  }
}

/** "Read 9 of 14 related notes". */
export function coverageLabel(plan: AskPlan): string {
  return `Read ${plan.sources.length} of ${plan.relatedNotes} related ${plan.relatedNotes === 1 ? 'note' : 'notes'}`;
}

const CITATION = /^\[(\d+(?:\s*,\s*\d+)*)\]/;

/**
 * Renders an answer as Markdown that can only show text and citations.
 *
 * Note excerpts can carry text written by anyone (a clipped web page), and that text can
 * steer the model. Images and links are the ways an answer could send note content to
 * someone else's server, so neither renders: links stay as their literal Markdown text, and
 * raw HTML is escaped. `[n]` markers become citation buttons; a marker naming no source is
 * removed, so a citation always points at a note that was actually read.
 */
export function createAnswerRenderer(MarkdownItClass: typeof MarkdownIt) {
  const md = new MarkdownItClass({ html: false, linkify: false });
  md.disable(['image', 'link', 'autolink', 'reference', 'html_inline', 'html_block']);
  md.inline.ruler.before('text', 'citation', (state, silent) => {
    if (state.src.charCodeAt(state.pos) !== 0x5b /* [ */) return false;
    const match = CITATION.exec(state.src.slice(state.pos));
    if (!match) return false;
    if (!silent) {
      const token = state.push('citation', '', 0);
      token.meta = { numbers: match[1].split(',').map((value) => Number(value.trim())) };
    }
    state.pos += match[0].length;
    return true;
  });
  md.renderer.rules.citation = (tokens, index, _options, env: { sourceCount: number }) => {
    const valid = (tokens[index].meta.numbers as number[]).filter(
      (number) => number >= 1 && number <= env.sourceCount,
    );
    return valid
      .map((number) => `<button type="button" class="citation" data-source="${number}">[${number}]</button>`)
      .join('');
  };
  return (text: string, sourceCount: number): string => md.render(text, { sourceCount });
}

/** Why Ask cannot run, or null when it can. */
export function askUnavailableReason(provider: string | null | undefined): string | null {
  return provider ? null : 'No AI provider is set up for Ask';
}

/** What a key pressed in the question box does in Ask mode. Escape closes the overlay first. */
export function askInputKey(key: string, shiftKey: boolean): 'submit' | 'focus-sources' | null {
  if (key === 'Enter') return 'submit';
  if (key === 'ArrowDown' || (key === 'Tab' && !shiftKey)) return 'focus-sources';
  return null;
}

/** The source ↑/↓ moves to from `index`, staying within the list; null for other keys. */
export function nextSourceIndex(index: number, count: number, key: string): number | null {
  if (index < 0 || count === 0) return null;
  if (key === 'ArrowDown') return Math.min(index + 1, count - 1);
  if (key === 'ArrowUp') return Math.max(index - 1, 0);
  return null;
}

/** Records `path` as gone when opening it found no note, so its row can say so. */
export function withMissing(missing: Set<string>, path: string, outcome: string): Set<string> {
  return outcome === 'not-found' ? new Set(missing).add(path) : missing;
}

/** Unread notes shown before "Show all": a broad question can leave thousands unread. */
export const UNREAD_SHOWN = 25;

export function visibleUnread<T>(unread: T[], showAll: boolean): T[] {
  return showAll ? unread : unread.slice(0, UNREAD_SHOWN);
}

/**
 * Opens a cited note, first checking it still exists on disk. Navigation skips its read when
 * the note is already open, so without the check a note deleted outside the app would
 * "open" and close Ask instead of showing that it is gone.
 */
export async function openCitation(
  path: string,
  read: (path: string) => Promise<unknown>,
  open: (path: string) => Promise<string>,
): Promise<string> {
  try {
    await read(path);
  } catch {
    return 'not-found';
  }
  return open(path);
}

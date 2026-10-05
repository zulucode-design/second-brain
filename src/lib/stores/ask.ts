import { get, writable } from 'svelte/store';
import type { UnlistenFn } from '@tauri-apps/api/event';
import { askNotes, cancelAi } from '$lib/api';
import { listenAppEvent } from '$lib/events';
import type { ParaCategory } from '$lib/types';
import { isRunning, keepLatest, type AskAnswer } from '$lib/utils/ask';

/**
 * Ask's latest answers, newest first. Held in memory only, so quitting drops them; they
 * live here rather than in the search overlay so opening a citation, which closes the
 * overlay, does not lose them.
 */
export const askAnswers = writable<AskAnswer[]>([]);

const listeners = new Map<string, UnlistenFn>();

/** Applies `change` to a running answer; a stopped, finished, or dropped one stays as it is. */
function patch(id: string, change: (answer: AskAnswer) => Partial<AskAnswer>) {
  askAnswers.update((answers) =>
    answers.map((answer) =>
      answer.id === id && isRunning(answer) ? { ...answer, ...change(answer) } : answer,
    ),
  );
}

function isLive(id: string): boolean {
  return get(askAnswers).some((answer) => answer.id === id && isRunning(answer));
}

function stopListening(id: string) {
  listeners.get(id)?.();
  listeners.delete(id);
}

export async function ask(question: string, category: ParaCategory | undefined, vault: string) {
  const id = crypto.randomUUID();
  const { kept, dropped } = keepLatest(get(askAnswers), {
    id,
    vault,
    question,
    status: 'searching',
    plan: null,
    text: '',
    error: null,
    stageStartedAt: Date.now(),
  });
  askAnswers.set(kept);
  for (const answer of dropped) {
    if (isRunning(answer)) void stop(answer.id);
  }

  // Listen before asking: the backend starts streaming before the plan comes back.
  const unlisten = await listenAppEvent('aiStream', ({ payload }) => {
    if (payload.request_id !== id) return;
    if (payload.event_type === 'text' && payload.text) {
      const text = payload.text;
      patch(id, (answer) => ({ status: 'answering', text: answer.text + text }));
    } else if (payload.event_type === 'thinking') {
      patch(id, (answer) =>
        answer.status === 'answering' ? {} : { status: 'thinking', stageStartedAt: Date.now() },
      );
    } else if (payload.event_type === 'done') {
      patch(id, () => ({ status: 'done' }));
      stopListening(id);
    } else if (payload.event_type === 'error') {
      patch(id, () => ({ status: 'error', error: payload.error ?? 'The answer failed.' }));
      stopListening(id);
    }
  });
  listeners.set(id, unlisten);
  if (!isLive(id)) {
    // Stopped or dropped while the listener was being set up.
    stopListening(id);
    return;
  }

  try {
    const plan = await askNotes(question, category, id);
    const current = get(askAnswers).find((answer) => answer.id === id);
    if (!current || current.status === 'stopped') {
      // Stopped or dropped during retrieval, before the stream existed to cancel.
      if (plan.sources.length > 0) await cancelAi(id);
      return;
    }
    if (plan.sources.length === 0) {
      patch(id, () => ({ status: 'no-match', plan }));
      stopListening(id);
      return;
    }
    // A fast stream can finish before the plan arrives, so the plan attaches either way.
    askAnswers.update((answers) =>
      answers.map((answer) => {
        if (answer.id !== id) return answer;
        return answer.status === 'searching'
          ? { ...answer, plan, status: 'reading', stageStartedAt: Date.now() }
          : { ...answer, plan };
      }),
    );
  } catch (error) {
    patch(id, () => ({ status: 'error', error: String(error) }));
    stopListening(id);
  }
}

export async function stop(id: string) {
  patch(id, () => ({ status: 'stopped' }));
  stopListening(id);
  try {
    await cancelAi(id);
  } catch (error) {
    console.error('Failed to stop the answer:', error);
  }
}

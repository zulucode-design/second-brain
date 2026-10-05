import type { Writable } from 'svelte/store';
import type { AiStreamEvent, AskPlan, ParaCategory } from '$lib/types';
import { forVault, isRunning, keepLatest, type AskAnswer } from '$lib/utils/ask';

/** What an Ask session needs from the app; tests pass fakes. */
export interface AskSessionDeps {
  answers: Writable<AskAnswer[]>;
  askNotes(question: string, category: ParaCategory | undefined, requestId: string): Promise<AskPlan>;
  cancelAi(requestId: string): Promise<void>;
  /** Subscribes to every AI stream event; resolves to the unsubscribe function. */
  listen(handler: (event: AiStreamEvent) => void): Promise<() => void>;
  newId(): string;
  now(): number;
}

/**
 * Runs Ask's questions against `deps.answers`: one stream listener per question, Stop,
 * and dropping the old vault's answers on a switch.
 */
export function createAskSession(deps: AskSessionDeps) {
  const listeners = new Map<string, () => void>();

  function current(): AskAnswer[] {
    let answers: AskAnswer[] = [];
    deps.answers.subscribe((value) => (answers = value))();
    return answers;
  }

  /** Applies `change` to a running answer; a stopped, finished, or dropped one stays as it is. */
  function patch(id: string, change: (answer: AskAnswer) => Partial<AskAnswer>) {
    deps.answers.update((answers) =>
      answers.map((answer) =>
        answer.id === id && isRunning(answer) ? { ...answer, ...change(answer) } : answer,
      ),
    );
  }

  function stopListening(id: string) {
    listeners.get(id)?.();
    listeners.delete(id);
  }

  function cancelQuietly(id: string) {
    deps.cancelAi(id).catch((error) => console.error('Failed to stop the answer:', error));
  }

  function fail(id: string, error: unknown) {
    patch(id, () => ({ status: 'error', error: String(error) }));
    stopListening(id);
  }

  function onEvent(id: string, event: AiStreamEvent) {
    if (event.request_id !== id) return;
    if (event.event_type === 'text' && event.text) {
      const text = event.text;
      patch(id, (answer) => ({ status: 'answering', text: answer.text + text }));
    } else if (event.event_type === 'thinking') {
      patch(id, (answer) =>
        answer.status === 'answering' ? {} : { status: 'thinking', stageStartedAt: deps.now() },
      );
    } else if (event.event_type === 'done') {
      patch(id, () => ({ status: 'done' }));
      stopListening(id);
    } else if (event.event_type === 'error') {
      fail(id, event.error ?? 'The answer failed.');
    }
  }

  async function ask(question: string, category: ParaCategory | undefined, vault: string) {
    const id = deps.newId();
    const { kept, dropped } = keepLatest(current(), {
      id,
      vault,
      question,
      status: 'searching',
      plan: null,
      text: '',
      error: null,
      stageStartedAt: deps.now(),
    });
    deps.answers.set(kept);
    for (const answer of dropped) {
      stopListening(answer.id);
      if (isRunning(answer)) cancelQuietly(answer.id);
    }

    try {
      // Listen before asking: the backend starts streaming before the plan comes back.
      const unlisten = await deps.listen((event) => onEvent(id, event));
      if (!current().some((answer) => answer.id === id && isRunning(answer))) {
        unlisten(); // Stopped or dropped while the listener was being set up.
        return;
      }
      listeners.set(id, unlisten);

      const plan = await deps.askNotes(question, category, id);
      const answer = current().find((item) => item.id === id);
      if (!answer || answer.status === 'stopped') {
        // Stopped or dropped during retrieval. The backend already skips the model when the
        // Stop reached it in time; this covers one that arrived after.
        if (plan.sources.length > 0) cancelQuietly(id);
        return;
      }
      if (plan.sources.length === 0) {
        patch(id, () => ({ status: 'no-match', plan }));
        stopListening(id);
        return;
      }
      // A fast stream can finish before the plan arrives, so the plan attaches either way.
      deps.answers.update((answers) =>
        answers.map((item) => {
          if (item.id !== id) return item;
          return item.status === 'searching'
            ? { ...item, plan, status: 'reading', stageStartedAt: deps.now() }
            : { ...item, plan };
        }),
      );
    } catch (error) {
      fail(id, error);
    }
  }

  function stop(id: string) {
    patch(id, () => ({ status: 'stopped' }));
    stopListening(id);
    cancelQuietly(id);
  }

  /** Drops every answer from another vault, stopping any still running. */
  function vaultChanged(vault: string | null | undefined) {
    const { kept, dropped } = forVault(current(), vault);
    if (dropped.length === 0) return;
    deps.answers.set(kept);
    for (const answer of dropped) {
      stopListening(answer.id);
      if (isRunning(answer)) cancelQuietly(answer.id);
    }
  }

  return { ask, stop, vaultChanged };
}

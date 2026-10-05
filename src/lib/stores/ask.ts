import { writable } from 'svelte/store';
import { askNotes, cancelAi } from '$lib/api';
import { listenAppEvent } from '$lib/events';
import { appConfig } from '$lib/stores/app';
import type { AskAnswer } from '$lib/utils/ask';
import { createAskSession } from '$lib/utils/ask-session';

/**
 * Ask's latest answers, newest first. Held in memory only, so quitting drops them; they
 * live here rather than in the search overlay so opening a citation, which closes the
 * overlay, does not lose them.
 */
export const askAnswers = writable<AskAnswer[]>([]);

const session = createAskSession({
  answers: askAnswers,
  askNotes,
  cancelAi,
  listen: (handler) => listenAppEvent('aiStream', ({ payload }) => handler(payload)),
  newId: () => crypto.randomUUID(),
  now: () => Date.now(),
});

export const { ask, stop } = session;

appConfig.subscribe((config) => session.vaultChanged(config?.active_vault));

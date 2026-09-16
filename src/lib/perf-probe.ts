// Opt-in performance probe for the external-alpha budgets (issue #88). The backend enables it
// only when SECOND_BRAIN_PERF_LOG is set; otherwise every function here returns immediately.
// Samples carry timings and counts only, never note content, titles, paths, or queries.
import { invoke } from '@tauri-apps/api/core';

let enabled: Promise<boolean> | null = null;

function probeEnabled(): Promise<boolean> {
	enabled ??= invoke<boolean>('perf_probe_enabled').catch(() => false);
	return enabled;
}

function record(sample: Record<string, unknown>) {
	void invoke('record_perf_sample', { sample }).catch(() => {});
}

/** Resolves with performance.now() once the frame that follows this call has been painted. */
export function afterNextPaint(): Promise<number> {
	return new Promise((resolve) => {
		requestAnimationFrame(() => {
			const channel = new MessageChannel();
			channel.port1.onmessage = () => resolve(performance.now());
			channel.port2.postMessage(null);
		});
	});
}

/** Records the moment startup restoration finished and its result reached the screen. */
export async function markStartupReady() {
	const painted = afterNextPaint();
	if (!(await probeEnabled())) return;
	const paintedAt = await painted;
	const editable = document.querySelector('.editor-content[contenteditable="true"]');
	record({
		kind: 'startup-ready',
		sidebar: !!document.querySelector('.sidebar'),
		noteList: !!document.querySelector('.note-list'),
		editorEditable: !!editable,
		sinceNavigationMs: paintedAt,
	});
}

let editorProbeInstalled = false;

/** Times each ordinary key press in the rich editor from the input event to the next paint. */
export async function installEditorKeyProbe() {
	if (editorProbeInstalled || !(await probeEnabled())) return;
	editorProbeInstalled = true;
	document.addEventListener('keydown', (event) => {
		const target = event.target as HTMLElement | null;
		if (event.key.length !== 1 || event.ctrlKey || event.metaKey || event.altKey) return;
		if (!target?.closest('.editor-content[contenteditable="true"]')) return;
		void afterNextPaint().then((painted) => record({ kind: 'editor-key', ms: painted - event.timeStamp }));
	}, { capture: true });
}

/** Starts a timer; call the returned function after the view has drawn its result. */
export function startTimer(kind: string): (details?: Record<string, unknown>) => void {
	const started = performance.now();
	void probeEnabled();
	return (details = {}) => {
		// Request the paint before any await so the probe cannot miss the frame it measures.
		void Promise.all([probeEnabled(), afterNextPaint()]).then(([on, painted]) => {
			if (on) record({ kind, ms: painted - started, ...details });
		});
	};
}

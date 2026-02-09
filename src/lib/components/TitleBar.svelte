<script lang="ts">
	import { getCurrentWindow } from '@tauri-apps/api/window';
	import { vaultReady, focusMode, updateAvailable, showSettings, settingsTab } from '$lib/stores/app';

	let { onNewNote = () => {} }: {
		onNewNote?: () => void;
	} = $props();

	const appWindow = getCurrentWindow();
	let maximized = $state(false);

	async function checkMaximized() {
		maximized = await appWindow.isMaximized();
	}

	$effect(() => {
		checkMaximized();
		const unlisten = appWindow.onResized(() => checkMaximized());
		return () => { unlisten.then(fn => fn()); };
	});

	let lastMouseDown = 0;

	const RESIZE_EDGE = 6;

	function handleMouseDown(e: MouseEvent) {
		if (e.button !== 0) return;
		const target = e.target as HTMLElement;
		if (target.closest('.titlebar-controls') || target.closest('.titlebar-actions')) return;

		// Don't start dragging near window edges — let Tauri handle resize
		if (!maximized) {
			const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
			if (e.clientY - rect.top < RESIZE_EDGE || e.clientX - rect.left < RESIZE_EDGE) return;
		}

		const now = Date.now();
		if (now - lastMouseDown < 300) {
			// Double-click detected — maximize/restore
			appWindow.toggleMaximize();
			lastMouseDown = 0;
			return;
		}
		lastMouseDown = now;
		appWindow.startDragging();
	}

	async function minimize() {
		await appWindow.minimize();
	}

	async function toggleMaximize() {
		await appWindow.toggleMaximize();
	}

	async function close() {
		await appWindow.close();
	}
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="titlebar" onmousedown={handleMouseDown}>
	<div class="titlebar-brand">
		<svg width="18" height="18" viewBox="0 0 48 48" fill="none">
			<rect width="48" height="48" rx="12" fill="var(--accent)" />
			<circle cx="16" cy="16" r="3.5" fill="white" opacity="0.9" />
			<circle cx="32" cy="16" r="3.5" fill="white" opacity="0.9" />
			<circle cx="16" cy="32" r="3.5" fill="white" opacity="0.9" />
			<circle cx="32" cy="32" r="3.5" fill="white" opacity="0.9" />
			<line x1="19" y1="18" x2="29" y2="30" stroke="white" stroke-width="2" stroke-linecap="round" opacity="0.7" />
			<line x1="29" y1="18" x2="19" y2="30" stroke="white" stroke-width="2" stroke-linecap="round" opacity="0.7" />
		</svg>
		<span class="titlebar-title">HelixNotes</span>
		{#if $updateAvailable}
			<!-- svelte-ignore a11y_no_static_element_interactions -->
		<button class="update-badge" onmousedown={(e) => e.stopPropagation()} onclick={() => { $settingsTab = 'updates'; $showSettings = true; }}>
				v{$updateAvailable.version} available
			</button>
		{/if}
	</div>
	<div class="titlebar-actions">
		<button class="switch-vault-btn" onclick={() => ($vaultReady = false)} title="Switch Vault">
			<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
				<path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z" />
			</svg>
		</button>
		<button class="switch-vault-btn" onclick={() => ($focusMode = true)} title="Focus mode">
			<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
				<path d="M8 3H5a2 2 0 00-2 2v3m18 0V5a2 2 0 00-2-2h-3m0 18h3a2 2 0 002-2v-3M3 16v3a2 2 0 002 2h3"/>
			</svg>
		</button>
		<button class="new-note-btn" onclick={onNewNote} title="New Note (Ctrl+N)">
			<svg width="14" height="14" viewBox="0 0 14 14" fill="none">
				<path d="M7 1v12M1 7h12" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" />
			</svg>
			New Note
		</button>
	</div>
	<div class="titlebar-controls">
		<button class="titlebar-btn" onclick={minimize} title="Minimize">
			<svg width="10" height="10" viewBox="0 0 10 10">
				<line x1="1" y1="5" x2="9" y2="5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" />
			</svg>
		</button>
		<button class="titlebar-btn" onclick={toggleMaximize} title={maximized ? 'Restore' : 'Maximize'}>
			{#if maximized}
				<svg width="10" height="10" viewBox="0 0 10 10">
					<rect x="2.5" y="0.5" width="7" height="7" rx="1" fill="none" stroke="currentColor" stroke-width="1.2" />
					<rect x="0.5" y="2.5" width="7" height="7" rx="1" fill="var(--bg-secondary)" stroke="currentColor" stroke-width="1.2" />
				</svg>
			{:else}
				<svg width="10" height="10" viewBox="0 0 10 10">
					<rect x="1" y="1" width="8" height="8" rx="1" fill="none" stroke="currentColor" stroke-width="1.2" />
				</svg>
			{/if}
		</button>
		<button class="titlebar-btn titlebar-close" onclick={close} title="Close">
			<svg width="10" height="10" viewBox="0 0 10 10">
				<line x1="1.5" y1="1.5" x2="8.5" y2="8.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" />
				<line x1="8.5" y1="1.5" x2="1.5" y2="8.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" />
			</svg>
		</button>
	</div>
</div>

<style>
	.titlebar {
		display: flex;
		align-items: center;
		height: 34px;
		background: var(--bg-secondary);
		border-bottom: 1px solid var(--border-color);
		user-select: none;
		flex-shrink: 0;
		-webkit-app-region: drag;
	}

	.titlebar-brand {
		display: flex;
		align-items: center;
		gap: 8px;
		padding-left: 14px;
		pointer-events: none;
		flex: 1;
	}

	.titlebar-title {
		font-size: 12px;
		font-weight: 500;
		color: var(--text-tertiary);
	}

	.update-badge {
		font-size: 10px;
		font-weight: 600;
		color: var(--accent);
		background: color-mix(in srgb, var(--accent) 12%, transparent);
		border: 1px solid color-mix(in srgb, var(--accent) 25%, transparent);
		border-radius: 10px;
		padding: 1px 8px;
		cursor: pointer;
		pointer-events: auto;
		-webkit-app-region: no-drag;
		transition: background 0.15s, border-color 0.15s;
	}

	.update-badge:hover {
		background: color-mix(in srgb, var(--accent) 20%, transparent);
		border-color: color-mix(in srgb, var(--accent) 40%, transparent);
	}

	.titlebar-actions {
		display: flex;
		align-items: center;
		gap: 6px;
		margin-right: 8px;
		-webkit-app-region: no-drag;
	}

	.switch-vault-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 28px;
		height: 28px;
		border: none;
		border-radius: 7px;
		background: var(--bg-hover);
		color: var(--text-secondary);
		cursor: pointer;
		transition: background 0.15s, color 0.15s;
	}

	.switch-vault-btn:hover {
		background: var(--bg-tertiary, var(--bg-hover));
		color: var(--text-primary);
	}

	.new-note-btn {
		display: flex;
		align-items: center;
		gap: 5px;
		padding: 4px 12px 4px 10px;
		border: none;
		border-radius: 7px;
		background: var(--accent);
		color: white;
		font-size: 11.5px;
		font-weight: 600;
		letter-spacing: 0.01em;
		cursor: pointer;
		white-space: nowrap;
		box-shadow: 0 1px 3px rgba(0, 0, 0, 0.15);
		transition: background 0.15s, box-shadow 0.15s, transform 0.1s;
	}

	.new-note-btn:hover {
		background: var(--accent-hover);
		box-shadow: 0 2px 6px rgba(0, 0, 0, 0.2);
	}

	.new-note-btn:active {
		transform: scale(0.97);
		box-shadow: 0 1px 2px rgba(0, 0, 0, 0.15);
	}

	.titlebar-controls {
		display: flex;
		height: 100%;
		-webkit-app-region: no-drag;
	}

	.titlebar-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 38px;
		height: 100%;
		border: none;
		background: none;
		color: var(--text-tertiary);
		cursor: pointer;
		transition: background 0.1s, color 0.1s;
	}

	.titlebar-btn:hover {
		background: var(--bg-hover);
		color: var(--text-primary);
	}

	.titlebar-close:hover {
		background: #e81123;
		color: white;
	}
</style>

<script lang="ts">
	import { onDestroy } from 'svelte';
	import { open } from '@tauri-apps/plugin-dialog';
	import { getCurrentWindow } from '@tauri-apps/api/window';
	import { endVaultSwitch, getAppConfig, openVault } from '$lib/api';
	import { appConfig, vaultReady } from '$lib/stores/app';

	let { initialError = '' }: { initialError?: string } = $props();
	let loading = $state(false);
	let error = $state('');
	let selectedFolder = $state<string | null>(null);
	let confirmedLocalFolder = $state(false);
	let switchReleaseStarted = false;

	$effect(() => {
		if (initialError && !error) error = initialError;
	});

	async function releaseVaultSwitch() {
		if (switchReleaseStarted) return;
		await endVaultSwitch();
		switchReleaseStarted = true;
	}

	async function returnToVault() {
		if (loading) return;
		try {
			await releaseVaultSwitch();
			$vaultReady = true;
		} catch (cause) {
			error = `Could not resume vault: ${cause}`;
		}
	}

	async function chooseFolder() {
		if (loading) return;
		try {
			const selected = await open({ directory: true, multiple: false, title: 'Choose Second Brain vault folder' });
			if (selected) {
				selectedFolder = selected as string;
				confirmedLocalFolder = false;
				error = '';
			}
		} catch (cause) {
			error = String(cause);
		}
	}

	async function openSelectedFolder() {
		if (loading || !selectedFolder || !confirmedLocalFolder) return;
		loading = true;
		error = '';
		try {
			await openVault(selectedFolder);
			$appConfig = await getAppConfig();
			await releaseVaultSwitch();
			$vaultReady = true;
		} catch (cause) {
			error = String(cause);
		} finally {
			loading = false;
		}
	}

	onDestroy(() => {
		void releaseVaultSwitch().catch((cause) => console.error('Could not release vault-switch gate:', cause));
	});
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="vault-picker" onmousedown={(event) => {
	if (!(event.target as HTMLElement).closest('button, input, label, .picker-card')) getCurrentWindow().startDragging();
}}>
	<button class="window-close" onclick={() => getCurrentWindow().close()} title="Close" aria-label="Close Second Brain">×</button>
	<div class="picker-card">
		<h1>Second Brain</h1>
		<p>Choose one local folder for your notes. An existing Markdown vault can be opened here.</p>
		{#if $appConfig?.active_vault}
			<p class="current">Current vault: <strong>{$appConfig.active_vault}</strong></p>
		{:else if $appConfig?.vault}
			<p class="current">Previous vault: <strong>{$appConfig.vault.path}</strong></p>
			<button class="back" onclick={() => { selectedFolder = $appConfig?.vault?.path ?? null; confirmedLocalFolder = false; }} disabled={loading}>Select previous folder</button>
		{/if}
		<p>Use a local disk or directly attached drive. Folders managed by OneDrive, Dropbox, Nextcloud, or another sync app are unsupported; Second Brain manages its own sync.</p>
		<button class="primary" onclick={chooseFolder} disabled={loading}>Choose vault folder</button>
		{#if selectedFolder}
			<p class="current">Selected: <strong>{selectedFolder}</strong></p>
			<label class="confirm"><input type="checkbox" bind:checked={confirmedLocalFolder} /> This folder is not managed by another sync app.</label>
			<button class="primary" onclick={openSelectedFolder} disabled={loading || !confirmedLocalFolder}>
				{loading ? 'Opening…' : 'Open selected folder'}
			</button>
		{/if}
		{#if error}<p class="error" role="alert">{error}</p>{/if}
		{#if $appConfig?.active_vault}
			<button class="back" onclick={returnToVault} disabled={loading}>Back to current vault</button>
		{/if}
	</div>
</div>

<style>
	.vault-picker { min-height: 100dvh; display: grid; place-items: center; position: relative; background: var(--bg-primary); color: var(--text-primary); }
	.picker-card { width: min(480px, calc(100vw - 32px)); padding: 32px; border: 1px solid var(--border-color); border-radius: 12px; background: var(--bg-secondary); box-shadow: 0 16px 48px #0002; }
	h1 { margin: 0 0 16px; font-size: 24px; }
	p { line-height: 1.5; color: var(--text-secondary); }
	.current { overflow-wrap: anywhere; }
	.confirm { display: flex; gap: 8px; align-items: flex-start; margin: 24px 0; line-height: 1.4; cursor: pointer; }
	.confirm input { margin-top: 3px; }
	.error { color: var(--error, #b91c1c); }
	button { font: inherit; cursor: pointer; }
	.primary, .back { display: block; width: 100%; padding: 10px 14px; border-radius: 7px; }
	.primary { border: 0; background: var(--accent); color: white; }
	.primary:disabled { opacity: .5; cursor: not-allowed; }
	.back { margin-top: 10px; border: 1px solid var(--border-color); background: transparent; color: inherit; }
	.window-close { position: absolute; top: 8px; right: 8px; width: 32px; height: 32px; border: 0; background: transparent; color: inherit; font-size: 24px; }
</style>

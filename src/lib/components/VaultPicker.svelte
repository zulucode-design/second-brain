<script lang="ts">
	import { open } from '@tauri-apps/plugin-dialog';
	import { getCurrentWindow } from '@tauri-apps/api/window';
	import { openVault, getAppConfig } from '$lib/api';
	import { appConfig, vaultReady } from '$lib/stores/app';
	import type { VaultConfig } from '$lib/types';

	const appWindow = getCurrentWindow();
	const isMobile = /android|ios/i.test(navigator.userAgent);

	let recentVaults: VaultConfig[] = $derived($appConfig?.vaults ?? []);
	let loading = $state(false);
	let error = $state('');

	async function pickFolder() {
		if (isMobile) {
			// Use shared Documents folder for Syncthing/Nextcloud compatibility
			const vaultPath = '/storage/emulated/0/Documents/HelixNotes';
			await openSelectedVault(vaultPath);
		} else {
			const selected = await open({ directory: true, multiple: false, title: 'Choose Notes Folder' });
			if (selected) {
				await openSelectedVault(selected as string);
			}
		}
	}

	async function openSelectedVault(path: string) {
		loading = true;
		error = '';
		try {
			await openVault(path);
			const config = await getAppConfig();
			$appConfig = config;
			$vaultReady = true;
		} catch (e) {
			error = String(e);
		} finally {
			loading = false;
		}
	}
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="vault-picker" class:mobile={isMobile} onmousedown={(e) => { if (!isMobile && !(e.target as HTMLElement).closest('button, .picker-card')) appWindow.startDragging(); }}>
	{#if !isMobile}
	<div class="window-controls">
		<button class="window-close" onclick={() => appWindow.close()} title="Close">
			<svg width="10" height="10" viewBox="0 0 10 10">
				<line x1="1.5" y1="1.5" x2="8.5" y2="8.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" />
				<line x1="8.5" y1="1.5" x2="1.5" y2="8.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" />
			</svg>
		</button>
	</div>
	{/if}
	<div class="picker-card">
		<div class="logo">
			<svg width="72" height="72" viewBox="0 0 48 48" fill="none">
				<rect width="48" height="48" rx="12" fill="var(--accent)" />
				<circle cx="16" cy="16" r="3.5" fill="white" opacity="0.9" />
				<circle cx="32" cy="16" r="3.5" fill="white" opacity="0.9" />
				<circle cx="16" cy="32" r="3.5" fill="white" opacity="0.9" />
				<circle cx="32" cy="32" r="3.5" fill="white" opacity="0.9" />
				<line x1="19" y1="18" x2="29" y2="30" stroke="white" stroke-width="2" stroke-linecap="round" opacity="0.7" />
				<line x1="29" y1="18" x2="19" y2="30" stroke="white" stroke-width="2" stroke-linecap="round" opacity="0.7" />
			</svg>
		</div>
		<h1>HelixNotes</h1>
		<p class="subtitle">Local markdown notes</p>
		{#if isMobile}
			<p class="description">Notes are stored in Documents/HelixNotes as Markdown files. Sync with Nextcloud, Syncthing, or any file sync app.</p>
		{:else}
			<p class="description">Your notes are stored as standard Markdown (.md) files. Pick any folder — existing .md files will be recognized automatically.</p>
		{/if}

		{#if error}
			<div class="error">{error}</div>
		{/if}

		<button class="btn-primary" onclick={pickFolder} disabled={loading}>
			{#if loading}
				Opening...
			{:else if isMobile}
				Get Started
			{:else}
				Open Notes Folder
			{/if}
		</button>

		{#if !isMobile && $appConfig?.active_vault}
			<button class="btn-back" onclick={() => ($vaultReady = true)}>
				Back
			</button>
		{/if}

		{#if !isMobile && recentVaults.length > 0}
			<div class="recent">
				<span class="recent-label">Recent</span>
				{#each recentVaults as vault}
					<button class="vault-item" onclick={() => openSelectedVault(vault.path)}>
						<span class="vault-name">{vault.name}</span>
						<span class="vault-path">{vault.path}</span>
					</button>
				{/each}
			</div>
		{/if}
	</div>
</div>

<style>
	.vault-picker {
		display: flex;
		align-items: center;
		justify-content: center;
		height: 100vh;
		background: var(--bg-primary);
		position: relative;
	}

	.window-controls {
		position: absolute;
		top: 8px;
		right: 8px;
		z-index: 10;
	}

	.window-close {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 32px;
		height: 28px;
		border: none;
		background: none;
		color: var(--text-tertiary);
		cursor: pointer;
		border-radius: 6px;
		transition: background 0.1s, color 0.1s;
	}

	.window-close:hover {
		background: #e81123;
		color: white;
	}

	.picker-card {
		text-align: center;
		max-width: 400px;
		width: 100%;
		padding: 48px 32px;
	}

	.logo {
		margin-bottom: 16px;
		display: flex;
		justify-content: center;
	}

	h1 {
		font-size: 24px;
		font-weight: 700;
		margin-bottom: 4px;
		color: var(--text-primary);
	}

	.subtitle {
		color: var(--text-tertiary);
		margin-bottom: 8px;
		font-size: 14px;
	}

	.description {
		color: var(--text-tertiary);
		font-size: 12px;
		line-height: 1.5;
		margin-bottom: 24px;
		opacity: 0.8;
	}

	.error {
		background: color-mix(in srgb, var(--danger) 10%, transparent);
		color: var(--danger);
		padding: 8px 12px;
		border-radius: 6px;
		margin-bottom: 16px;
		font-size: 13px;
	}

	.btn-primary {
		width: 100%;
		padding: 10px 20px;
		background: var(--accent);
		color: white;
		border: none;
		border-radius: 8px;
		font-size: 14px;
		font-weight: 500;
		cursor: pointer;
		transition: background 0.15s;
	}

	.btn-primary:hover {
		background: var(--accent-hover);
	}

	.btn-primary:disabled {
		opacity: 0.6;
		cursor: not-allowed;
	}

	.btn-back {
		width: 100%;
		padding: 10px 20px;
		background: var(--bg-tertiary);
		color: var(--text-secondary);
		border: 1px solid var(--border-color);
		border-radius: 8px;
		font-size: 14px;
		font-weight: 500;
		cursor: pointer;
		margin-top: 8px;
		transition: background 0.15s, color 0.15s;
	}

	.btn-back:hover {
		background: var(--bg-hover);
		color: var(--text-primary);
	}

	.recent {
		margin-top: 32px;
		text-align: left;
	}

	.recent-label {
		font-size: 11px;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		color: var(--text-tertiary);
		display: block;
		margin-bottom: 8px;
	}

	.vault-item {
		display: block;
		width: 100%;
		padding: 10px 12px;
		background: var(--bg-secondary);
		border: 1px solid var(--border-color);
		border-radius: 8px;
		cursor: pointer;
		text-align: left;
		margin-bottom: 6px;
		transition: background 0.15s;
	}

	.vault-item:hover {
		background: var(--bg-hover);
	}

	.vault-name {
		display: block;
		font-weight: 500;
		font-size: 14px;
		color: var(--text-primary);
	}

	.vault-path {
		display: block;
		font-size: 12px;
		color: var(--text-tertiary);
		margin-top: 2px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	/* Mobile */
	.mobile .btn-primary {
		padding: 14px 20px;
		font-size: 16px;
		border-radius: 12px;
	}

	.mobile .description {
		font-size: 14px;
	}

	.mobile h1 {
		font-size: 28px;
	}

	.mobile .logo :global(svg) {
		width: 88px;
		height: 88px;
	}
</style>

<script lang="ts">
	import { onMount } from 'svelte';
	import { appConfig, vaultReady, theme } from '$lib/stores/app';
	import { getAppConfig, openVault } from '$lib/api';
	import VaultPicker from '$lib/components/VaultPicker.svelte';
	import AppLayout from '$lib/components/AppLayout.svelte';

	let loading = $state(true);

	onMount(async () => {
		try {
			const config = await getAppConfig();
			$appConfig = config;
			$theme = config.theme || 'system';

			// Apply theme immediately to prevent flash
			const themeValue = config.theme || 'system';
			const root = document.documentElement;
			root.classList.remove('dark');
			if (themeValue === 'dark' || (themeValue === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches)) {
				root.classList.add('dark');
			}

			// Apply saved font settings
			if (config.font_size) {
				document.documentElement.style.setProperty('--editor-font-size', `${config.font_size}px`);
			}
			if (config.font_family) {
				const fontStacks: Record<string, string> = {
					system: '-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif',
					inter: '"Inter", -apple-system, BlinkMacSystemFont, sans-serif',
					georgia: 'Georgia, "Times New Roman", serif',
					merriweather: '"Merriweather", Georgia, serif',
					lora: '"Lora", Georgia, serif',
					opensans: '"Open Sans", -apple-system, sans-serif',
					literata: '"Literata", Georgia, serif',
					mono: '"JetBrains Mono", "Fira Code", "Cascadia Code", monospace',
				};
				const stack = fontStacks[config.font_family];
				if (stack) document.documentElement.style.setProperty('--editor-font-family', stack);
			}
			// Apply saved accent
			if (config.accent_color) {
				const accentPresets: Record<string, { light: string; dark: string }> = {
					Indigo: { light: '#5b6abf', dark: '#7b9bd4' },
					Rose: { light: '#e11d48', dark: '#d4768a' },
					Emerald: { light: '#059669', dark: '#5bab8a' },
					Amber: { light: '#d97706', dark: '#c9a04e' },
					Purple: { light: '#7c3aed', dark: '#9a82c4' },
					Cyan: { light: '#0891b2', dark: '#5ba8b5' },
					Orange: { light: '#ea580c', dark: '#c98560' },
					Teal: { light: '#0d9488', dark: '#5fada5' },
				};
				const preset = accentPresets[config.accent_color];
				if (preset) {
					const themeVal = config.theme || 'light';
					const isDark = themeVal === 'dark' || (themeVal === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches);
					const color = isDark ? preset.dark : preset.light;
					const root = document.documentElement;
					root.style.setProperty('--accent', color);
					root.style.setProperty('--text-accent', color);
					root.style.setProperty('--accent-hover', color);
					root.style.setProperty('--accent-light', isDark
						? `color-mix(in srgb, ${color} 10%, transparent)`
						: `color-mix(in srgb, ${color} 8%, transparent)`);
				}
			}

			// Auto-open last vault if available
			if (config.active_vault) {
				try {
					await openVault(config.active_vault);
					$vaultReady = true;
				} catch {
					// Vault path might not exist anymore
				}
			}
		} catch {
			// First launch or no config
		}
		loading = false;
	});
</script>

{#if loading}
	<div class="loading">
		<div class="spinner"></div>
	</div>
{:else if $vaultReady}
	<AppLayout />
{:else}
	<VaultPicker />
{/if}

<style>
	.loading {
		display: flex;
		align-items: center;
		justify-content: center;
		height: 100vh;
		background: var(--bg-primary);
	}

	.spinner {
		width: 24px;
		height: 24px;
		border: 2px solid var(--border-color);
		border-top-color: var(--accent);
		border-radius: 50%;
		animation: spin 0.6s linear infinite;
	}

	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}
</style>

<script lang="ts">
	import { onMount } from 'svelte';
	import '../app.css';
	import { theme, appConfig, activeNotePath, installType, checkForUpdate, checkForUpdateMobile } from '$lib/stores/app';
	import { openUrl } from '@tauri-apps/plugin-opener';
	import { openFile, getInstallType } from '$lib/api';
	import { get } from 'svelte/store';

	let { children } = $props();

	const isMobile = /android|ios/i.test(navigator.userAgent);

	// Reactively apply theme class to <html> whenever $theme changes
	$effect(() => {
		const t = $theme;
		const root = document.documentElement;
		root.classList.remove('dark');
		if (
			t === 'dark' ||
			(t === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches)
		) {
			root.classList.add('dark');
		}
	});

	function normalizePath(p: string): string {
		const parts = p.split('/');
		const resolved: string[] = [];
		for (const seg of parts) {
			if (seg === '..') resolved.pop();
			else if (seg !== '.') resolved.push(seg);
		}
		return resolved.join('/');
	}

	// Detect install type and check for updates on startup
	onMount(() => {
		if (isMobile) {
			installType.set('android');
			checkForUpdateMobile();
		} else {
			getInstallType().then(t => installType.set(t)).catch(() => {});
			checkForUpdate();
		}
	});

	// Intercept all link clicks in capture phase to prevent webview navigation
	onMount(() => {
		function handleLinkClick(e: MouseEvent) {
			const target = (e.target as HTMLElement)?.closest('a');
			if (!target) return;
			const href = target.getAttribute('href');
			if (!href) return;

			e.preventDefault();
			e.stopPropagation();
			e.stopImmediatePropagation();

			if (href.startsWith('http://') || href.startsWith('https://') || href.startsWith('mailto:')) {
				openUrl(href).catch((err) => console.error('Failed to open URL:', err));
			} else if (!href.startsWith('#')) {
				const decoded = decodeURIComponent(href);
				const config = get(appConfig);
				const vaultRoot = config?.active_vault;
				let absPath = decoded;
				if (!decoded.startsWith('/') && vaultRoot) {
					const notePath = get(activeNotePath);
					if (notePath) {
						const noteDir = notePath.substring(0, notePath.lastIndexOf('/'));
						absPath = normalizePath(`${noteDir}/${decoded}`);
					} else {
						absPath = normalizePath(`${vaultRoot}/${decoded}`);
					}
				}
				openFile(absPath).catch((err) => console.error('Failed to open file:', err));
			}
		}

		document.addEventListener('click', handleLinkClick, true);
		return () => document.removeEventListener('click', handleLinkClick, true);
	});
</script>

<svelte:document oncontextmenu={(e) => { if (!isMobile) e.preventDefault(); }} />

<svelte:head>
	<title>HelixNotes</title>
</svelte:head>

{@render children()}

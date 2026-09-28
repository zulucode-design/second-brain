<script lang="ts">
	import { onMount } from 'svelte';
	import '../app.css';
	import { resolvedTheme, appConfig, activeNotePath, compactLayout, customThemes } from '$lib/stores/app';
	import { openFile, openUrl } from '$lib/api';
	import { get } from 'svelte/store';
	import { darkThemes } from '$lib/platform';
	import type { CustomTheme } from '$lib/types';
	import ResizeHandles from '$lib/components/ResizeHandles.svelte';
	import { resolveVaultFilePath } from '$lib/utils/paths';
	import { requestNoteNavigation } from '$lib/utils/navigation';

	let { children } = $props();

	// The quick-capture overlay is its own window on the same bundle. It gets the theme, but
	// none of the main window's furniture because it is not resizable.
	const isCaptureWindow =
		typeof window !== 'undefined' && window.location.pathname.startsWith('/capture');

	// Reactively apply theme class to <html> whenever the resolved theme or custom themes change.
	// $resolvedTheme already maps "system" onto the configured light/dark pair, so flipping the OS
	// appearance re-runs this effect.
	$effect(() => {
		applyTheme($resolvedTheme, $customThemes);
	});

	// Apply link arrow visibility from config
	$effect(() => {
		if ($appConfig) {
			document.documentElement.classList.toggle('no-link-arrows', !$appConfig.show_link_arrows);
		}
	});

	const CUSTOM_THEME_VARS = [
		'--bg-primary', '--bg-secondary', '--bg-tertiary',
		'--bg-hover', '--bg-active', '--bg-editor',
		'--text-primary', '--text-secondary', '--border-color',
		'--border-light', '--text-tertiary',
	];

	// A custom theme carries nine colors, but the stylesheet also reads --border-light and
	// --text-tertiary. Named themes set those in their own :root[data-theme] block; a custom theme
	// has no such block, so without these they fall back to the light defaults in app.css and a dark
	// custom theme draws near-white dividers. Derive them from the closest color the theme does have.
	function applyCustomThemeVars(root: HTMLElement, ct: CustomTheme) {
		root.style.setProperty('--bg-primary', ct.colors.bg_primary);
		root.style.setProperty('--bg-secondary', ct.colors.bg_secondary);
		root.style.setProperty('--bg-tertiary', ct.colors.bg_tertiary);
		root.style.setProperty('--bg-hover', ct.colors.bg_hover);
		root.style.setProperty('--bg-active', ct.colors.bg_active);
		root.style.setProperty('--bg-editor', ct.colors.bg_editor);
		root.style.setProperty('--text-primary', ct.colors.text_primary);
		root.style.setProperty('--text-secondary', ct.colors.text_secondary);
		root.style.setProperty('--border-color', ct.colors.border_color);
		root.style.setProperty('--border-light', ct.colors.border_color);
		root.style.setProperty('--text-tertiary', ct.colors.text_secondary);
	}

	function clearCustomThemeVars(root: HTMLElement) {
		for (const v of CUSTOM_THEME_VARS) root.style.removeProperty(v);
	}

	function applyTheme(t: string, themes: CustomTheme[] = []) {
		const namedThemes = ['solarized-light', 'solarized-dark', 'catppuccin', 'nord', 'tokyo-night', 'github-light', 'github-dark', 'dracula', 'blueberry', 'forest-green', 'gruvbox', 'midnight-tide', 'cherry-blossom', 'synthwave', 'ember', 'moonlit', 'light-coffee', 'dark-coffee', 'cotton-candy', 'crimson', 'cloud', 'peach', 'material-dark', 'material-light', 'monokai', 'rose-pine', 'everforest', 'horizon', 'cyberpunk', 'black', 'one-dark'];
		const root = document.documentElement;
		root.classList.remove('dark');
		root.removeAttribute('data-theme');
		clearCustomThemeVars(root);
		if (t.startsWith('custom-')) {
			const ct = themes.find(c => c.id === t);
			if (ct) {
				applyCustomThemeVars(root, ct);
				if (ct.is_dark) root.classList.add('dark');
			}
		} else if (namedThemes.includes(t)) {
			root.setAttribute('data-theme', t);
			if (darkThemes.includes(t)) root.classList.add('dark');
		} else if (t === 'dark' || (t === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches)) {
			root.classList.add('dark');
		}
	}

	function openLocalFile(path: string) {
		openFile(path).catch((err) => console.error('Failed to open file:', err));
	}

	function resolveAndHandleLink(href: string) {
		if (href.startsWith('http://') || href.startsWith('https://') || href.startsWith('mailto:') || href.startsWith('tel:') || href.startsWith('sms:')) {
			openUrl(href).catch((err) => console.error('Failed to open URL:', err));
		} else if (!href.startsWith('#')) {
			const config = get(appConfig);
			const absPath = resolveVaultFilePath(
				decodeURIComponent(href),
				get(activeNotePath),
				config?.active_vault ?? null,
			);
			// Internal .md note link - navigate within the app
			if (absPath.endsWith('.md')) {
				requestNoteNavigation(absPath);
			} else {
				openLocalFile(absPath);
			}
		}
	}

	// Compact layout follows window width on both supported desktops.
	onMount(() => {
		const query = window.matchMedia('(max-width: 800px)');
		const update = () => compactLayout.set(query.matches);
		update();
		query.addEventListener('change', update);
		return () => query.removeEventListener('change', update);
	});

	// Intercept all link clicks in capture phase to prevent webview navigation
	onMount(() => {
		function handleLinkClick(e: MouseEvent) {
			const el = e.target as HTMLElement;
			// PDF/attachment embeds: check for data-open-file on target or ancestors
			const fileTarget = el?.closest('[data-open-file]') as HTMLElement | null;
			if (fileTarget) {
				e.preventDefault();
				e.stopPropagation();
				e.stopImmediatePropagation();
				openLocalFile(fileTarget.getAttribute('data-open-file')!);
				return;
			}
			const target = el?.closest('a');
			if (!target) return;
			const href = target.getAttribute('href');
			if (!href) return;
			e.preventDefault();
			e.stopPropagation();
			e.stopImmediatePropagation();
			resolveAndHandleLink(href);
		}

		document.addEventListener('click', handleLinkClick, true);


		return () => {
			document.removeEventListener('click', handleLinkClick, true);
		};
	});

	// On Windows the native OS drag-drop handler is disabled (dragDropEnabled:false
	// in tauri.windows.conf.json) so HTML5 drag-and-drop works for reordering. With it
	// off, a file dropped outside a drop zone would make the webview navigate to / open
	// that file, replacing the app. Swallow any drag that bubbles up unhandled. Real drop
	// zones (editor, sidebar reordering) call preventDefault in their own handlers first;
	// these bubble-phase listeners only act as a fallback. On Linux OS file drops are
	// intercepted natively and never surface as HTML5 events, so this is a harmless no-op there.
	onMount(() => {
		function preventNavigate(e: DragEvent) {
			e.preventDefault();
		}
		window.addEventListener('dragover', preventNavigate);
		window.addEventListener('drop', preventNavigate);
		return () => {
			window.removeEventListener('dragover', preventNavigate);
			window.removeEventListener('drop', preventNavigate);
		};
	});
</script>

<svelte:document oncontextmenu={(e) => { e.preventDefault(); }} />

<svelte:head>
	<title>Second Brain</title>
</svelte:head>

{@render children()}

{#if !isCaptureWindow}
	<ResizeHandles />
{/if}

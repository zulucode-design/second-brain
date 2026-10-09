<script lang="ts">
	import { onMount } from 'svelte';
	import '../app.css';
	import { resolvedTheme, appConfig, activeNotePath, compactLayout, customThemes } from '$lib/stores/app';
	import { openFile, openUrl } from '$lib/api';
	import { get } from 'svelte/store';
	import { applyTheme } from '$lib/theme';
	import ResizeHandles from '$lib/components/ResizeHandles.svelte';
	import { resolveVaultFilePath } from '$lib/utils/paths';
	import { requestNoteNavigation } from '$lib/utils/navigation';
	import { undoCommand } from '$lib/utils/text-undo';

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
		// Same breakpoint as the compact media query in app.css.
		const query = window.matchMedia('(max-width: 768px)');
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

	// Undo and redo in plain text fields, which the Linux WebView leaves unbound (#213). Bubble
	// phase, so a field that handles these keys itself goes first.
	onMount(() => {
		function undoInField(e: KeyboardEvent) {
			if (e.defaultPrevented) return;
			if (!(e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement)) return;
			const command = undoCommand(e);
			if (!command) return;
			e.preventDefault();
			document.execCommand(command);
		}
		window.addEventListener('keydown', undoInField);
		return () => window.removeEventListener('keydown', undoInField);
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

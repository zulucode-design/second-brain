<script lang="ts">
	import { onMount } from 'svelte';
	import '../app.css';
	import { theme, appConfig, activeNote, activeNotePath, installType, checkForUpdate, checkForUpdateMobile } from '$lib/stores/app';
	import { openUrl } from '@tauri-apps/plugin-opener';
	import { openFile, readNote, getInstallType } from '$lib/api';
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

	function resolveAndHandleLink(href: string) {
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
			// Internal .md note link — navigate within the app
			if (absPath.endsWith('.md')) {
				readNote(absPath).then((content) => {
					activeNote.set({ ...content, content: content.content });
					activeNotePath.set(absPath);
				}).catch((err) => console.error('Failed to navigate to note:', err));
			} else {
				openFile(absPath).catch((err) => console.error('Failed to open file:', err));
			}
		}
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
			resolveAndHandleLink(href);
		}

		document.addEventListener('click', handleLinkClick, true);

		// On Android WebView, tapping a link can trigger native navigation before
		// the JS click event fires (causing 404s for relative .md links).
		// We intercept touchend: if it's a tap (not a scroll) on an <a>, we
		// preventDefault to block the native navigation and handle it ourselves.
		let touchStartY = 0;
		function handleTouchStart(e: TouchEvent) {
			touchStartY = e.touches[0]?.clientY ?? 0;
		}
		function handleTouchEnd(e: TouchEvent) {
			const endY = e.changedTouches[0]?.clientY ?? 0;
			// If the finger moved > 10px, it's a scroll, not a tap
			if (Math.abs(endY - touchStartY) > 10) return;
			const target = (e.target as HTMLElement)?.closest('a');
			if (!target) return;
			const href = target.getAttribute('href');
			if (!href) return;
			e.preventDefault();
			resolveAndHandleLink(href);
		}

		if (isMobile) {
			document.addEventListener('touchstart', handleTouchStart, { capture: true, passive: true });
			document.addEventListener('touchend', handleTouchEnd, true);
		}

		return () => {
			document.removeEventListener('click', handleLinkClick, true);
			if (isMobile) {
				document.removeEventListener('touchstart', handleTouchStart, true);
				document.removeEventListener('touchend', handleTouchEnd, true);
			}
		};
	});
</script>

<svelte:document oncontextmenu={(e) => { if (!isMobile) e.preventDefault(); }} />

<svelte:head>
	<title>HelixNotes</title>
</svelte:head>

{@render children()}

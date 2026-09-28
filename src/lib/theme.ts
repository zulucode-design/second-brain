import type { CustomTheme } from '$lib/types';

// Themes that use the dark color scheme. Keep in sync with the
// `:root[data-theme]` blocks in app.css.
export const darkThemes = [
	'dark',
	'solarized-dark',
	'catppuccin',
	'nord',
	'tokyo-night',
	'github-dark',
	'dracula',
	'blueberry',
	'forest-green',
	'gruvbox',
	'midnight-tide',
	'cherry-blossom',
	'synthwave',
	'ember',
	'moonlit',
	'dark-coffee',
	'crimson',
	'material-dark',
	'monokai',
	'rose-pine',
	'everforest',
	'horizon',
	'cyberpunk',
	'black',
	'one-dark',
];

// Themes with their own `:root[data-theme]` block in app.css.
const NAMED_THEMES = ['solarized-light', 'solarized-dark', 'catppuccin', 'nord', 'tokyo-night', 'github-light', 'github-dark', 'dracula', 'blueberry', 'forest-green', 'gruvbox', 'midnight-tide', 'cherry-blossom', 'synthwave', 'ember', 'moonlit', 'light-coffee', 'dark-coffee', 'cotton-candy', 'crimson', 'cloud', 'peach', 'material-dark', 'material-light', 'monokai', 'rose-pine', 'everforest', 'horizon', 'cyberpunk', 'black', 'one-dark'];

// `theme` is a resolved theme id, never "system".
export function isDarkTheme(theme: string, customThemes: CustomTheme[] = []): boolean {
	return darkThemes.includes(theme) || (customThemes.find((c) => c.id === theme)?.is_dark ?? false);
}

// A custom theme carries nine colors, but the stylesheet also reads --border-light and
// --text-tertiary. Named themes set those in their own :root[data-theme] block; a custom theme
// has no such block, so without these they fall back to the light defaults in app.css and a dark
// custom theme draws near-white dividers. Derive them from the closest color the theme does have.
const CUSTOM_THEME_VARS: Record<string, keyof CustomTheme['colors']> = {
	'--bg-primary': 'bg_primary',
	'--bg-secondary': 'bg_secondary',
	'--bg-tertiary': 'bg_tertiary',
	'--bg-hover': 'bg_hover',
	'--bg-active': 'bg_active',
	'--bg-editor': 'bg_editor',
	'--text-primary': 'text_primary',
	'--text-secondary': 'text_secondary',
	'--border-color': 'border_color',
	'--border-light': 'border_color',
	'--text-tertiary': 'text_secondary',
};

export function applyTheme(t: string, themes: CustomTheme[] = []) {
	const root = document.documentElement;
	root.classList.remove('dark');
	root.removeAttribute('data-theme');
	for (const name of Object.keys(CUSTOM_THEME_VARS)) root.style.removeProperty(name);
	if (t.startsWith('custom-')) {
		const ct = themes.find(c => c.id === t);
		if (ct) {
			for (const [name, color] of Object.entries(CUSTOM_THEME_VARS)) root.style.setProperty(name, ct.colors[color]);
			if (ct.is_dark) root.classList.add('dark');
		}
	} else if (NAMED_THEMES.includes(t)) {
		root.setAttribute('data-theme', t);
		if (darkThemes.includes(t)) root.classList.add('dark');
	} else if (t === 'dark' || (t === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches)) {
		root.classList.add('dark');
	}
}

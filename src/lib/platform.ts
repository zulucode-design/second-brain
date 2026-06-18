// Centralized platform detection. iOS WebViews report iPhone/iPad/iPod in the user
// agent (not the literal "ios"), so they must be matched explicitly.
const ua = typeof navigator !== 'undefined' ? navigator.userAgent : '';

export const isAndroid = /android/i.test(ua);
export const isIOS = /iphone|ipad|ipod/i.test(ua);
export const isMobile = isAndroid || isIOS;

// Themes that use the dark color scheme. Used by applyTheme() to toggle the
// `dark` class on the root element. Keep this in sync when adding new themes.
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

export function isDarkTheme(theme: string): boolean {
	return darkThemes.includes(theme) || (theme === 'system' && typeof window !== 'undefined' && window.matchMedia('(prefers-color-scheme: dark)').matches);
}

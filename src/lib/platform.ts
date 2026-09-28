// Supported OS identity is injected by the backend before app scripts run.
type SupportedPlatform = { linux: boolean; windows: boolean };
const injected: SupportedPlatform | undefined =
  typeof window !== 'undefined'
    ? (window as unknown as { __SECOND_BRAIN_PLATFORM__?: SupportedPlatform }).__SECOND_BRAIN_PLATFORM__
    : undefined;

export const isLinux = injected?.linux ?? false;
export const isWindows = injected?.windows ?? false;

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

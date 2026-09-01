import type { ParaCategory, ViewMode } from '$lib/types';

function normalizeRelativePath(path: string): string {
	return path.replace(/\\/g, '/').replace(/^\/+|\/+$/g, '');
}

export function suggestedNotebookForCreation(
	viewMode: ViewMode,
	activeNotebookRelative: string | null | undefined
): string | null {
	if (viewMode !== 'notebook' || !activeNotebookRelative) return null;
	const normalized = normalizeRelativePath(activeNotebookRelative);
	const topLevel = normalized.split('/')[0];
	return ['Projects', 'Areas', 'Resources', 'Archives'].includes(topLevel)
		? normalized
		: null;
}

export function destinationForCategory(
	category: ParaCategory,
	suggestedNotebook: string | null
): string {
	if (!suggestedNotebook) return category;
	const normalized = normalizeRelativePath(suggestedNotebook);
	return normalized.split('/')[0] === category ? normalized : category;
}

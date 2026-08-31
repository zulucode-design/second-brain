const PARA_ROOTS = new Set(['Projects', 'Areas', 'Resources', 'Archives']);

function normalize(path: string): string {
	return path.replace(/\\/g, '/').replace(/^\/+|\/+$/g, '');
}

function parentOf(path: string): string {
	const normalized = normalize(path);
	const separator = normalized.lastIndexOf('/');
	return separator < 0 ? '' : normalized.slice(0, separator);
}

export function isParaCategoryRoot(relativePath: string): boolean {
	return PARA_ROOTS.has(normalize(relativePath));
}

export function isInsideParaCategory(relativePath: string): boolean {
	const [root] = normalize(relativePath).split('/');
	return PARA_ROOTS.has(root);
}

export function notebookUiPolicy(relativePath: string) {
	const categoryRoot = isParaCategoryRoot(relativePath);
	const insideCategory = isInsideParaCategory(relativePath);
	return {
		createChild: insideCategory,
		rename: !categoryRoot,
		delete: !categoryRoot,
		move: !categoryRoot,
		reorder: !categoryRoot && isInsideParaCategory(parentOf(relativePath)),
		setIcon: true
	};
}

export function canCreateNotebookUnder(parentRelativePath: string | null | undefined): boolean {
	return !!parentRelativePath && isInsideParaCategory(parentRelativePath);
}

export function canMoveNotebookTo(
	sourceRelativePath: string,
	destinationParentRelativePath: string
): boolean {
	return !isParaCategoryRoot(sourceRelativePath) && isInsideParaCategory(destinationParentRelativePath);
}

export function canReorderNotebookBeside(
	sourceRelativePath: string,
	targetRelativePath: string
): boolean {
	return canMoveNotebookTo(sourceRelativePath, parentOf(targetRelativePath));
}

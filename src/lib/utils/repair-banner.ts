import type { RepairStatus } from '$lib/types';

export interface RepairBanner {
	/** `repair` offers "Repair now"; `notice` only reports and offers "Dismiss". */
	kind: 'repair' | 'notice';
	title: string;
	message: string;
	path: string | null;
}

/**
 * What the vault banner shows. Real repairs outrank restore notices (#142), which report an
 * interrupted restore that was already recovered and need no action beyond reading them.
 */
export function repairBanner(status: RepairStatus, repairError: string): RepairBanner | null {
	const repairs = status.issues.filter((issue) => issue.stage !== 'restore');
	if (repairs.length > 0 || repairError) {
		return {
			kind: 'repair',
			title: 'Vault repair needed',
			message:
				repairs.length > 0
					? `${repairs.length} issue${repairs.length === 1 ? '' : 's'} may leave filing or search results incomplete.`
					: repairError,
			path: repairs[0]?.paths[0] ?? null
		};
	}
	const notice = status.issues.find((issue) => issue.stage === 'restore');
	if (!notice) return null;
	return {
		kind: 'notice',
		title: notice.key === 'restore:unowned' ? 'Restore folders found' : 'Restore interrupted',
		message: notice.message,
		path: notice.paths[0] ?? null
	};
}

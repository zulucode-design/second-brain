import type { RepairStatus } from '$lib/types';

export interface RepairBanner {
	/** `repair` offers "Repair now"; `notice` only reports and offers "Dismiss". */
	kind: 'repair' | 'notice';
	/** The notice to dismiss; repairs are cleared by repairing, not by key. */
	key: string | null;
	title: string;
	message: string;
	paths: string[];
}

/**
 * What the vault banner shows. Real repairs outrank restore notices (#142). Among notices,
 * unowned restore folders come first: they may hold the only copy of a vault's notes, so
 * they must be seen, with every path, before anything else is dismissed.
 */
export function repairBanner(status: RepairStatus, repairError: string): RepairBanner | null {
	const repairs = status.issues.filter((issue) => issue.stage !== 'restore');
	if (repairs.length > 0 || repairError) {
		return {
			kind: 'repair',
			key: null,
			title: 'Vault repair needed',
			message:
				repairs.length > 0
					? `${repairs.length} issue${repairs.length === 1 ? '' : 's'} may leave filing or search results incomplete.`
					: repairError,
			paths: repairs[0]?.paths.slice(0, 1) ?? []
		};
	}
	const notices = status.issues.filter((issue) => issue.stage === 'restore');
	const notice = notices.find((issue) => issue.key === 'restore:unowned') ?? notices[0];
	if (!notice) return null;
	return {
		kind: 'notice',
		key: notice.key,
		title: notice.key === 'restore:unowned' ? 'Restore folders found' : 'Restore interrupted',
		message: notice.message,
		paths: notice.paths
	};
}

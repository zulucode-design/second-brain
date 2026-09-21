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
 * What the vault banner shows (#142). Unowned restore folders come first, even before a
 * repair: they may hold the only copy of a vault's notes, so they must be seen, with every
 * path. Then repairs, then the notice that an interrupted restore was recovered.
 */
export function repairBanner(status: RepairStatus, repairError: string): RepairBanner | null {
	const notices = status.issues.filter((issue) => issue.stage === 'restore');
	const unowned = notices.find((issue) => issue.key === 'restore:unowned');
	if (unowned) return notice(unowned);
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
	return notices[0] ? notice(notices[0]) : null;
}

function notice(issue: RepairStatus['issues'][number]): RepairBanner {
	return {
		kind: 'notice',
		key: issue.key,
		title: issue.key === 'restore:unowned' ? 'Restore folders found' : 'Restore interrupted',
		message: issue.message,
		paths: issue.paths
	};
}

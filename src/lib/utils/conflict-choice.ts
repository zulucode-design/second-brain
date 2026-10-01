/**
 * Applies a sync conflict choice, then lists the conflicts again. The choice may have replaced
 * the open note, so `afterChoice` runs as soon as the choice settles, failed or not, without
 * waiting for the slower list (#191).
 */
export async function applyConflictChoice<T>(
	choose: () => Promise<void>,
	afterChoice: () => void,
	relist: () => Promise<T>,
): Promise<T> {
	try {
		await choose();
	} finally {
		afterChoice();
	}
	return relist();
}

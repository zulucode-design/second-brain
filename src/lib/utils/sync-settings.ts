import type { VaultConfig } from '$lib/types';

/** The sync settings the settings panel edits, in the shape the form holds them. */
export interface EditedSyncSettings {
	provider: string | null;
	url: string | null;
	username: string | null;
	password: string | null;
	onOpen: boolean;
	onChange: boolean;
	intervalMinutes: number;
}

/**
 * A copy of `vault` with the edited sync settings applied.
 *
 * Mirrors what the backend's `set_sync_settings` writes, so the store matches the config on
 * disk without waiting for a restart. Credentials stay scoped to their provider, so a
 * provider added later merges in here instead of adding fields alongside. `last_sync_time`
 * is the backend's to set and carries through untouched.
 */
export function withSyncSettings(vault: VaultConfig, edited: EditedSyncSettings): VaultConfig {
	return {
		...vault,
		sync_provider: edited.provider,
		credentials: {
			...vault.credentials,
			webdav: {
				url: edited.url,
				username: edited.username,
				password: edited.password
			}
		},
		schedule: {
			...vault.schedule,
			on_open: edited.onOpen,
			on_change: edited.onChange,
			interval_minutes: edited.intervalMinutes
		}
	};
}

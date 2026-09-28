<script lang="ts">
	import { showCommandPalette, showSearch, theme, viewMode, activeNotebook, activeTag } from '$lib/stores/app';
	import { setTheme, reindex } from '$lib/api';

	// onNavigate runs the parent's view-change handler (refreshes the note list and reveals it
	// if it was hidden), so opening a view here behaves exactly like clicking it in the sidebar.
	let { onNavigate = () => {}, onToggleSource }: { onNavigate?: () => void; onToggleSource: () => void } = $props();

	function openView(mode: 'all' | 'quickaccess' | 'tasks' | 'unfiled' | 'trash') {
		$viewMode = mode;
		$activeNotebook = null;
		$activeTag = null;
		$showCommandPalette = false;
		onNavigate();
	}

	function switchTheme(id: string) {
		$theme = id;
		setTheme(id);
		$showCommandPalette = false;
	}

	interface Command {
		id: string;
		label: string;
		shortcut?: string;
		action: () => void;
	}

	const modKey = 'Ctrl';

	let query = $state('');
	let selectedIndex = $state(0);
	let inputEl = $state<HTMLInputElement>(null!);

	const commands: Command[] = [
		{
			id: 'search',
			label: 'Search Notes',
			shortcut: `${modKey}+F`,
			action: () => {
				$showCommandPalette = false;
				$showSearch = true;
			}
		},
		{
			id: 'open-all-notes',
			label: 'Open All Notes',
			action: () => openView('all')
		},
		{
			id: 'open-quick-access',
			label: 'Open Quick Access',
			action: () => openView('quickaccess')
		},
		{
			id: 'open-tasks',
			label: 'Open Tasks',
			action: () => openView('tasks')
		},
		{
			id: 'open-unfiled',
			label: 'Open Unfiled Notes',
			action: () => openView('unfiled')
		},
		{
			id: 'open-trash',
			label: 'Open Trash (restore deleted notes)',
			action: () => openView('trash')
		},
		{
			id: 'theme-light',
			label: 'Switch to Light Theme',
			action: () => switchTheme('light')
		},
		{
			id: 'theme-dark',
			label: 'Switch to Dark Theme',
			action: () => switchTheme('dark')
		},
		{
			id: 'theme-system',
			label: 'Use System Theme',
			action: () => switchTheme('system')
		},
		{
			id: 'theme-solarized-light',
			label: 'Switch to Solarized Light Theme',
			action: () => switchTheme('solarized-light')
		},
		{
			id: 'theme-solarized-dark',
			label: 'Switch to Solarized Dark Theme',
			action: () => switchTheme('solarized-dark')
		},
		{
			id: 'theme-catppuccin',
			label: 'Switch to Catppuccin Theme',
			action: () => switchTheme('catppuccin')
		},
		{
			id: 'theme-nord',
			label: 'Switch to Nord Theme',
			action: () => switchTheme('nord')
		},
		{
			id: 'theme-tokyo-night',
			label: 'Switch to Tokyo Night Theme',
			action: () => switchTheme('tokyo-night')
		},
		{
			id: 'theme-github-light',
			label: 'Switch to GitHub Light Theme',
			action: () => switchTheme('github-light')
		},
		{
			id: 'theme-github-dark',
			label: 'Switch to GitHub Dark Theme',
			action: () => switchTheme('github-dark')
		},
		{
			id: 'theme-dracula',
			label: 'Switch to Dracula Theme',
			action: () => switchTheme('dracula')
		},
		{
			id: 'toggle-source',
			label: 'Toggle Source/WYSIWYG Mode',
			action: () => {
				onToggleSource();
				$showCommandPalette = false;
			}
		},
		{
			id: 'reindex',
			label: 'Rebuild Search Index',
			action: async () => {
				await reindex();
				$showCommandPalette = false;
			}
		}
	];

	let filteredCommands = $derived(
		query.trim()
			? commands.filter((c) => c.label.toLowerCase().includes(query.toLowerCase()))
			: commands
	);

	$effect(() => {
		if ($showCommandPalette && inputEl) {
			query = '';
			selectedIndex = 0;
			setTimeout(() => inputEl?.focus(), 50);
		}
	});

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			$showCommandPalette = false;
		} else if (e.key === 'ArrowDown') {
			e.preventDefault();
			selectedIndex = Math.min(selectedIndex + 1, filteredCommands.length - 1);
		} else if (e.key === 'ArrowUp') {
			e.preventDefault();
			selectedIndex = Math.max(selectedIndex - 1, 0);
		} else if (e.key === 'Enter' && filteredCommands.length > 0) {
			filteredCommands[selectedIndex].action();
		}
	}


</script>

{#if $showCommandPalette}
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div class="palette-overlay" onclick={() => ($showCommandPalette = false)} onkeydown={handleKeydown}>
		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<div class="palette-panel" onclick={(e) => e.stopPropagation()} onkeydown={(e) => e.stopPropagation()}>
			<div class="palette-input-wrapper">
				<svg width="16" height="16" viewBox="0 0 16 16" fill="var(--text-tertiary)">
					<path d="M3 2v4.586l7 7L14.586 9l-7-7H3zm2 1a1 1 0 110 2 1 1 0 010-2z" />
				</svg>
				<input
					bind:this={inputEl}
					type="text"
					placeholder="Type a command..."
					bind:value={query}
					onkeydown={handleKeydown}
				/>
			</div>

			<div class="palette-results">
				{#each filteredCommands as cmd, i (cmd.id)}
					<button
						class="cmd-item"
						class:selected={i === selectedIndex}
						onclick={() => cmd.action()}
						onmouseenter={() => (selectedIndex = i)}
					>
						<span class="cmd-label">{cmd.label}</span>
						{#if cmd.shortcut}
							<span class="cmd-shortcut">{cmd.shortcut}</span>
						{/if}
					</button>
				{/each}
			</div>
		</div>
	</div>
{/if}

<style>
	.palette-overlay {
		position: fixed;
		inset: 0;
		background: rgba(0, 0, 0, 0.3);
		display: flex;
		align-items: flex-start;
		justify-content: center;
		padding-top: 20vh;
		z-index: 2000;
	}

	.palette-panel {
		background: var(--bg-primary);
		border: 1px solid var(--border-color);
		border-radius: 12px;
		box-shadow: var(--shadow-lg);
		width: 480px;
		max-height: 360px;
		overflow: hidden;
		display: flex;
		flex-direction: column;
	}

	.palette-input-wrapper {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 14px 16px;
		border-bottom: 1px solid var(--border-light);
	}

	.palette-input-wrapper input {
		flex: 1;
		border: none;
		background: none;
		color: var(--text-primary);
		font-size: 15px;
		outline: none;
	}

	.palette-input-wrapper input::placeholder {
		color: var(--text-tertiary);
	}

	.palette-results {
		overflow-y: auto;
		padding: 4px;
	}

	.cmd-item {
		display: flex;
		align-items: center;
		justify-content: space-between;
		width: 100%;
		padding: 10px 14px;
		border: none;
		background: none;
		border-radius: 8px;
		cursor: pointer;
		text-align: left;
		transition: background 0.1s;
	}

	.cmd-item:hover,
	.cmd-item.selected {
		background: var(--bg-hover);
	}

	.cmd-label {
		font-size: 14px;
		color: var(--text-primary);
	}

	.cmd-shortcut {
		font-size: 12px;
		color: var(--text-tertiary);
		background: var(--bg-tertiary);
		padding: 2px 8px;
		border-radius: 4px;
	}
</style>

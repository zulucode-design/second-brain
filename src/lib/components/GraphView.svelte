<script lang="ts">
	import { onDestroy } from 'svelte';
	import { getAllNoteTitles, readNote } from '$lib/api';
	import { appConfig, activeNotePath } from '$lib/stores/app';
	import type { NoteTitleEntry } from '$lib/types';

	let { onclose, onnavigate }: {
		onclose: () => void;
		onnavigate: (path: string, title: string) => void;
	} = $props();

	let canvas = $state<HTMLCanvasElement>(null!);
	let loading = $state(true);

	interface GraphNode {
		id: string;
		title: string;
		path: string;
		x: number;
		y: number;
		vx: number;
		vy: number;
		links: string[]; // titles this note links to
	}

	interface GraphEdge {
		source: string;
		target: string;
	}

	let nodes: GraphNode[] = [];
	let edges: GraphEdge[] = [];
	let animFrame = 0;
	let pan = { x: 0, y: 0 };
	let zoom = 1;
	let dragging: GraphNode | null = null;
	let dragMoved = false;
	let mouseDownPos = { x: 0, y: 0 };
	let panning = false;
	let panStart = { x: 0, y: 0 };
	let hoveredNode: GraphNode | null = null;

	const wikiLinkRegex = /\[\[([^\]]+)\]\]/g;

	async function buildGraph() {
		loading = true;
		try {
			const titles = await getAllNoteTitles();
			const titleMap = new Map<string, NoteTitleEntry>();
			for (const t of titles) {
				titleMap.set(t.title.toLowerCase(), t);
			}

			// Create nodes
			const w = canvas?.width ?? 800;
			const h = canvas?.height ?? 600;
			nodes = titles.map((t, i) => ({
				id: t.title.toLowerCase(),
				title: t.title,
				path: t.path,
				x: w / 2 + (Math.random() - 0.5) * Math.min(w, h) * 0.6,
				y: h / 2 + (Math.random() - 0.5) * Math.min(w, h) * 0.6,
				vx: 0,
				vy: 0,
				links: [],
			}));

			const nodeMap = new Map<string, GraphNode>();
			for (const n of nodes) nodeMap.set(n.id, n);

			// Read each note to extract [[wiki-links]]
			const edgeSet = new Set<string>();
			for (const node of nodes) {
				try {
					const content = await readNote(node.path);
					const body = content.content || '';
					let match;
					wikiLinkRegex.lastIndex = 0;
					while ((match = wikiLinkRegex.exec(body)) !== null) {
						const linkTitle = match[1].trim().toLowerCase();
						if (linkTitle !== node.id && nodeMap.has(linkTitle)) {
							node.links.push(linkTitle);
							const edgeKey = [node.id, linkTitle].sort().join('|');
							if (!edgeSet.has(edgeKey)) {
								edgeSet.add(edgeKey);
								edges.push({ source: node.id, target: linkTitle });
							}
						}
					}
				} catch {
					// Skip notes that can't be read
				}
			}
		} catch (e) {
			console.error('Failed to build graph:', e);
		}
		loading = false;
		startSimulation();
	}

	function fitToView() {
		if (!canvas || nodes.length === 0) return;
		const w = canvas.width;
		const h = canvas.height;
		const padding = 60;

		// Compute bounding box of all nodes
		let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
		for (const node of nodes) {
			if (node.x < minX) minX = node.x;
			if (node.y < minY) minY = node.y;
			if (node.x > maxX) maxX = node.x;
			if (node.y > maxY) maxY = node.y;
		}

		const graphW = maxX - minX || 1;
		const graphH = maxY - minY || 1;
		const centerGraphX = (minX + maxX) / 2;
		const centerGraphY = (minY + maxY) / 2;

		// Compute zoom to fit
		zoom = Math.min(
			(w - padding * 2) / graphW,
			(h - padding * 2) / graphH,
			2 // max zoom
		);
		zoom = Math.max(zoom, 0.2);

		// Center the graph
		pan.x = w / 2 - centerGraphX * zoom;
		pan.y = h / 2 - centerGraphY * zoom;
	}

	function startSimulation() {
		if (animFrame) cancelAnimationFrame(animFrame);
		let iterations = 0;
		const maxIterations = 300;

		function tick() {
			if (iterations >= maxIterations) {
				fitToView();
				draw();
				return;
			}
			simulate();
			draw();
			iterations++;
			animFrame = requestAnimationFrame(tick);
		}
		animFrame = requestAnimationFrame(tick);
	}

	function simulate() {
		const nodeCount = nodes.length;
		if (nodeCount === 0) return;

		const w = canvas?.width ?? 800;
		const h = canvas?.height ?? 600;
		const centerX = w / 2;
		const centerY = h / 2;

		// Repulsion between all nodes
		for (let i = 0; i < nodeCount; i++) {
			for (let j = i + 1; j < nodeCount; j++) {
				const a = nodes[i];
				const b = nodes[j];
				let dx = b.x - a.x;
				let dy = b.y - a.y;
				let dist = Math.sqrt(dx * dx + dy * dy) || 1;
				const force = 800 / (dist * dist);
				const fx = (dx / dist) * force;
				const fy = (dy / dist) * force;
				a.vx -= fx;
				a.vy -= fy;
				b.vx += fx;
				b.vy += fy;
			}
		}

		// Attraction along edges
		for (const edge of edges) {
			const a = nodes.find(n => n.id === edge.source);
			const b = nodes.find(n => n.id === edge.target);
			if (!a || !b) continue;
			let dx = b.x - a.x;
			let dy = b.y - a.y;
			let dist = Math.sqrt(dx * dx + dy * dy) || 1;
			const force = (dist - 100) * 0.01;
			const fx = (dx / dist) * force;
			const fy = (dy / dist) * force;
			a.vx += fx;
			a.vy += fy;
			b.vx -= fx;
			b.vy -= fy;
		}

		// Center gravity
		for (const node of nodes) {
			node.vx += (centerX - node.x) * 0.001;
			node.vy += (centerY - node.y) * 0.001;
		}

		// Apply velocity with damping
		for (const node of nodes) {
			if (node === dragging) continue;
			node.vx *= 0.85;
			node.vy *= 0.85;
			node.x += node.vx;
			node.y += node.vy;
		}
	}

	function draw() {
		if (!canvas) return;
		const ctx = canvas.getContext('2d');
		if (!ctx) return;
		const w = canvas.width;
		const h = canvas.height;

		ctx.clearRect(0, 0, w, h);
		ctx.save();
		ctx.translate(pan.x, pan.y);
		ctx.scale(zoom, zoom);

		const style = getComputedStyle(document.documentElement);
		const borderColor = style.getPropertyValue('--border-color').trim() || '#444';
		const textColor = style.getPropertyValue('--text-primary').trim() || '#eee';
		const textSecondary = style.getPropertyValue('--text-tertiary').trim() || '#888';
		const accent = style.getPropertyValue('--accent').trim() || '#7b9bd4';
		const accentLight = style.getPropertyValue('--accent-light').trim() || 'rgba(123,155,212,0.15)';

		// Draw edges
		ctx.strokeStyle = borderColor;
		ctx.lineWidth = 1;
		ctx.globalAlpha = 0.4;
		for (const edge of edges) {
			const a = nodes.find(n => n.id === edge.source);
			const b = nodes.find(n => n.id === edge.target);
			if (!a || !b) continue;
			ctx.beginPath();
			ctx.moveTo(a.x, a.y);
			ctx.lineTo(b.x, b.y);
			ctx.stroke();
		}
		ctx.globalAlpha = 1;

		// Determine active note
		const activePath = $activeNotePath || '';

		// Draw nodes
		for (const node of nodes) {
			const isActive = node.path === activePath;
			const isHovered = node === hoveredNode;
			const hasLinks = node.links.length > 0 || edges.some(e => e.source === node.id || e.target === node.id);
			const radius = isActive ? 7 : hasLinks ? 5 : 3.5;

			// Node circle
			ctx.beginPath();
			ctx.arc(node.x, node.y, radius, 0, Math.PI * 2);
			if (isActive) {
				ctx.fillStyle = accent;
			} else if (isHovered) {
				ctx.fillStyle = accent;
			} else if (hasLinks) {
				ctx.fillStyle = textSecondary;
			} else {
				ctx.fillStyle = borderColor;
			}
			ctx.fill();

			// Label
			if (isActive || isHovered || hasLinks) {
				ctx.font = `${isActive || isHovered ? '12' : '10'}px -apple-system, BlinkMacSystemFont, sans-serif`;
				ctx.fillStyle = isActive || isHovered ? textColor : textSecondary;
				ctx.textAlign = 'center';
				ctx.fillText(node.title, node.x, node.y - radius - 5);
			}
		}

		ctx.restore();
	}

	function getNodeAt(clientX: number, clientY: number): GraphNode | null {
		if (!canvas) return null;
		const rect = canvas.getBoundingClientRect();
		const x = (clientX - rect.left - pan.x) / zoom;
		const y = (clientY - rect.top - pan.y) / zoom;
		for (let i = nodes.length - 1; i >= 0; i--) {
			const n = nodes[i];
			const dx = n.x - x;
			const dy = n.y - y;
			if (dx * dx + dy * dy < 100) return n;
		}
		return null;
	}

	function handleMouseDown(e: MouseEvent) {
		mouseDownPos = { x: e.clientX, y: e.clientY };
		dragMoved = false;
		const node = getNodeAt(e.clientX, e.clientY);
		if (node) {
			dragging = node;
		} else {
			panning = true;
			panStart = { x: e.clientX - pan.x, y: e.clientY - pan.y };
		}
	}

	function handleMouseMove(e: MouseEvent) {
		const dx = e.clientX - mouseDownPos.x;
		const dy = e.clientY - mouseDownPos.y;
		if (Math.abs(dx) > 3 || Math.abs(dy) > 3) {
			dragMoved = true;
		}

		if (dragging && dragMoved) {
			const rect = canvas.getBoundingClientRect();
			dragging.x = (e.clientX - rect.left - pan.x) / zoom;
			dragging.y = (e.clientY - rect.top - pan.y) / zoom;
			dragging.vx = 0;
			dragging.vy = 0;
			draw();
		} else if (panning) {
			pan.x = e.clientX - panStart.x;
			pan.y = e.clientY - panStart.y;
			draw();
		} else if (!dragging) {
			const node = getNodeAt(e.clientX, e.clientY);
			if (node !== hoveredNode) {
				hoveredNode = node;
				if (canvas) canvas.style.cursor = node ? 'pointer' : 'grab';
				draw();
			}
		}
	}

	function handleMouseUp(e: MouseEvent) {
		if (dragging && !dragMoved) {
			// It was a click, not a drag — navigate
			const node = dragging;
			dragging = null;
			onnavigate(node.path, node.title);
			return;
		}
		dragging = null;
		panning = false;
	}

	function handleWheel(e: WheelEvent) {
		e.preventDefault();
		const rect = canvas.getBoundingClientRect();
		const mx = e.clientX - rect.left;
		const my = e.clientY - rect.top;
		const oldZoom = zoom;
		const delta = e.deltaY > 0 ? 0.9 : 1.1;
		zoom = Math.max(0.2, Math.min(5, zoom * delta));
		// Zoom toward mouse position
		pan.x = mx - (mx - pan.x) * (zoom / oldZoom);
		pan.y = my - (my - pan.y) * (zoom / oldZoom);
		draw();
	}

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Escape') onclose();
	}

	$effect(() => {
		if (canvas) {
			const rect = canvas.parentElement?.getBoundingClientRect();
			if (rect) {
				canvas.width = rect.width;
				canvas.height = rect.height;
			}
			buildGraph();
		}
	});

	onDestroy(() => {
		if (animFrame) cancelAnimationFrame(animFrame);
	});
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="graph-overlay" onkeydown={handleKeydown}>
	<div class="graph-panel">
		<div class="graph-header">
			<h3>Graph View</h3>
			<div class="graph-stats">
				{#if !loading}
					{nodes.length} notes, {edges.length} connections
				{/if}
			</div>
			<button class="graph-close" onclick={onclose}>
				<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
					<line x1="18" y1="6" x2="6" y2="18" /><line x1="6" y1="6" x2="18" y2="18" />
				</svg>
			</button>
		</div>
		<div class="graph-body">
			{#if loading}
				<div class="graph-loading">
					<svg class="spinner" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
						<circle cx="12" cy="12" r="10" opacity="0.25" />
						<path d="M12 2a10 10 0 019.95 9" />
					</svg>
					Building graph...
				</div>
			{/if}
			<!-- svelte-ignore a11y_no_static_element_interactions -->
			<canvas
				bind:this={canvas}
				class="graph-canvas"
				onmousedown={handleMouseDown}
				onmousemove={handleMouseMove}
				onmouseup={handleMouseUp}
				onwheel={handleWheel}
			></canvas>
		</div>
	</div>
</div>

<style>
	.graph-overlay {
		position: fixed;
		inset: 0;
		background: rgba(0, 0, 0, 0.5);
		display: flex;
		align-items: center;
		justify-content: center;
		z-index: 2000;
	}

	.graph-panel {
		background: var(--bg-primary);
		border: 1px solid var(--border-color);
		border-radius: 16px;
		box-shadow: var(--shadow-lg);
		width: 85vw;
		height: 75vh;
		max-width: 1200px;
		max-height: 800px;
		display: flex;
		flex-direction: column;
		overflow: hidden;
	}

	.graph-header {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 14px 20px;
		border-bottom: 1px solid var(--border-light);
		flex-shrink: 0;
	}

	.graph-header h3 {
		font-size: 15px;
		font-weight: 600;
		color: var(--text-primary);
		margin: 0;
	}

	.graph-stats {
		flex: 1;
		font-size: 12px;
		color: var(--text-tertiary);
	}

	.graph-close {
		background: none;
		border: none;
		color: var(--text-tertiary);
		cursor: pointer;
		padding: 4px;
		border-radius: 6px;
		display: flex;
		align-items: center;
	}

	.graph-close:hover {
		background: var(--bg-hover);
		color: var(--text-primary);
	}

	.graph-body {
		flex: 1;
		position: relative;
		overflow: hidden;
	}

	.graph-canvas {
		width: 100%;
		height: 100%;
		cursor: grab;
	}

	.graph-canvas:active {
		cursor: grabbing;
	}

	.graph-loading {
		position: absolute;
		inset: 0;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 12px;
		color: var(--text-tertiary);
		font-size: 13px;
		z-index: 1;
	}

	.spinner {
		animation: spin 0.8s linear infinite;
	}

	@keyframes spin {
		from { transform: rotate(0deg); }
		to { transform: rotate(360deg); }
	}
</style>

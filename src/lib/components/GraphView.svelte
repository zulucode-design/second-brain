<script lang="ts">
	import { onDestroy } from 'svelte';
	import { getGraphData } from '$lib/api';
	import { activeNotePath } from '$lib/stores/app';

	let { onclose, onnavigate }: {
		onclose: () => void;
		onnavigate: (path: string, title: string) => void;
	} = $props();

	let canvas = $state<HTMLCanvasElement>(null!);
	let loading = $state(true);

	// Start fetching data immediately — don't wait for canvas mount
	const dataPromise = getGraphData();

	interface GraphNode {
		id: string;
		title: string;
		path: string;
		x: number;
		y: number;
		vx: number;
		vy: number;
	}

	interface GraphEdge {
		sourceIdx: number;
		targetIdx: number;
	}

	let nodes: GraphNode[] = [];
	let edges: GraphEdge[] = [];
	let nodeIndexMap: Map<string, number> = new Map();
	let connectedSet: Set<number> = new Set();
	let pan = { x: 0, y: 0 };
	let zoom = 1;
	let dragging: GraphNode | null = null;
	let dragMoved = false;
	let mouseDownPos = { x: 0, y: 0 };
	let panning = false;
	let panStart = { x: 0, y: 0 };
	let hoveredNode: GraphNode | null = null;
	let glowPhase = 0;
	let glowFrame = 0;

	// Cache computed styles — read once, not every frame
	let cachedStyles: { border: string; text: string; textSec: string; accent: string } | null = null;

	function getStyles() {
		if (cachedStyles) return cachedStyles;
		const style = getComputedStyle(document.documentElement);
		cachedStyles = {
			border: style.getPropertyValue('--border-color').trim() || '#444',
			text: style.getPropertyValue('--text-primary').trim() || '#eee',
			textSec: style.getPropertyValue('--text-tertiary').trim() || '#888',
			accent: style.getPropertyValue('--accent').trim() || '#7b9bd4',
		};
		return cachedStyles;
	}

	async function buildGraph() {
		loading = true;
		try {
			// Use the pre-fetched promise (started before canvas mount)
			const data = await dataPromise;

			const w = canvas?.width ?? 800;
			const h = canvas?.height ?? 600;

			// Build node index map
			nodeIndexMap = new Map();
			nodes = data.nodes.map((n, i) => {
				nodeIndexMap.set(n.title.toLowerCase(), i);
				return {
					id: n.title.toLowerCase(),
					title: n.title,
					path: n.path,
					x: w / 2 + (Math.random() - 0.5) * Math.min(w, h) * 0.6,
					y: h / 2 + (Math.random() - 0.5) * Math.min(w, h) * 0.6,
					vx: 0,
					vy: 0,
				};
			});

			// Map edges and track connected nodes
			connectedSet = new Set();
			edges = data.edges.map(e => {
				connectedSet.add(e.source);
				connectedSet.add(e.target);
				return { sourceIdx: e.source, targetIdx: e.target };
			});
		} catch (e) {
			console.error('Failed to build graph:', e);
		}
		loading = false;
		startSimulation();
	}

	function centerOnActiveNote() {
		if (!canvas || nodes.length === 0) return;
		const activePath = $activeNotePath || '';
		const activeNode = nodes.find(n => n.path === activePath);
		if (!activeNode) return;

		const activeIdx = nodeIndexMap.get(activeNode.id);

		// Gather the active node and its direct neighbors
		const neighborhood: GraphNode[] = [activeNode];
		if (activeIdx !== undefined) {
			for (const edge of edges) {
				if (edge.sourceIdx === activeIdx) neighborhood.push(nodes[edge.targetIdx]);
				else if (edge.targetIdx === activeIdx) neighborhood.push(nodes[edge.sourceIdx]);
			}
		}

		const w = canvas.width;
		const h = canvas.height;
		const padding = 80;

		let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
		for (const n of neighborhood) {
			if (n.x < minX) minX = n.x;
			if (n.y < minY) minY = n.y;
			if (n.x > maxX) maxX = n.x;
			if (n.y > maxY) maxY = n.y;
		}

		const graphW = maxX - minX || 1;
		const graphH = maxY - minY || 1;
		const centerX = (minX + maxX) / 2;
		const centerY = (minY + maxY) / 2;

		zoom = Math.min(
			(w - padding * 2) / graphW,
			(h - padding * 2) / graphH,
			1.8
		);
		zoom = Math.max(zoom, 0.5);

		pan.x = w / 2 - centerX * zoom;
		pan.y = h / 2 - centerY * zoom;
	}

	function fitToView() {
		if (!canvas || nodes.length === 0) return;
		const w = canvas.width;
		const h = canvas.height;
		const padding = 60;

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

		zoom = Math.min(
			(w - padding * 2) / graphW,
			(h - padding * 2) / graphH,
			2
		);
		zoom = Math.max(zoom, 0.2);

		pan.x = w / 2 - centerGraphX * zoom;
		pan.y = h / 2 - centerGraphY * zoom;
	}

	function startSimulation() {
		// Run a small batch synchronously for an instant first render
		for (let i = 0; i < 30; i++) simulate();

		// Center/fit immediately so the user sees something right away
		const activePath = $activeNotePath || '';
		const activeNode = nodes.find(n => n.path === activePath);
		if (activeNode) {
			centerOnActiveNote();
		} else {
			fitToView();
		}

		draw();
		startGlowLoop();

		// Continue settling asynchronously in small batches
		const totalRemaining = Math.min(270, Math.max(70, nodes.length * 2));
		let done = 0;
		function settle() {
			if (done >= totalRemaining) return;
			const batch = Math.min(20, totalRemaining - done);
			for (let i = 0; i < batch; i++) simulate();
			done += batch;
			// Re-center while settling
			if (activeNode) centerOnActiveNote(); else fitToView();
			requestAnimationFrame(settle);
		}
		requestAnimationFrame(settle);
	}

	function startGlowLoop() {
		if (glowFrame) cancelAnimationFrame(glowFrame);
		function loop() {
			glowPhase += 0.04;
			draw();
			glowFrame = requestAnimationFrame(loop);
		}
		glowFrame = requestAnimationFrame(loop);
	}

	function simulate() {
		const nodeCount = nodes.length;
		if (nodeCount === 0) return;

		const w = canvas?.width ?? 800;
		const h = canvas?.height ?? 600;
		const centerX = w / 2;
		const centerY = h / 2;

		// Repulsion between all nodes (skip pairs that are very far apart)
		for (let i = 0; i < nodeCount; i++) {
			const a = nodes[i];
			for (let j = i + 1; j < nodeCount; j++) {
				const b = nodes[j];
				const dx = b.x - a.x;
				const dy = b.y - a.y;
				const distSq = dx * dx + dy * dy;
				// Skip very distant pairs — negligible force
				if (distSq > 250000) continue;
				const d = distSq || 1;
				const force = 800 / d;
				const dist = Math.sqrt(d);
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
			const a = nodes[edge.sourceIdx];
			const b = nodes[edge.targetIdx];
			const dx = b.x - a.x;
			const dy = b.y - a.y;
			const dist = Math.sqrt(dx * dx + dy * dy) || 1;
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

		const { border: borderColor, text: textColor, textSec: textSecondary, accent } = getStyles();

		// Draw edges
		ctx.strokeStyle = borderColor;
		ctx.lineWidth = 1;
		ctx.globalAlpha = 0.4;
		ctx.beginPath();
		for (const edge of edges) {
			const a = nodes[edge.sourceIdx];
			const b = nodes[edge.targetIdx];
			ctx.moveTo(a.x, a.y);
			ctx.lineTo(b.x, b.y);
		}
		ctx.stroke();
		ctx.globalAlpha = 1;

		// Determine active note
		const activePath = $activeNotePath || '';

		// Draw nodes
		const pulse = 0.5 + 0.5 * Math.sin(glowPhase);

		for (let i = 0; i < nodes.length; i++) {
			const node = nodes[i];
			const isActive = node.path === activePath;
			const isHovered = node === hoveredNode;
			const hasLinks = connectedSet.has(i);
			const baseRadius = isActive ? 9 : hasLinks ? 5 : 3.5;
			const radius = isActive ? baseRadius + pulse * 2 : baseRadius;

			// Active node glow
			if (isActive) {
				const glowRadius = radius + 10 + pulse * 6;
				const glow = ctx.createRadialGradient(node.x, node.y, radius, node.x, node.y, glowRadius);
				glow.addColorStop(0, accent + '60');
				glow.addColorStop(1, accent + '00');
				ctx.beginPath();
				ctx.arc(node.x, node.y, glowRadius, 0, Math.PI * 2);
				ctx.fillStyle = glow;
				ctx.fill();
			}

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

			// Active node ring
			if (isActive) {
				ctx.beginPath();
				ctx.arc(node.x, node.y, radius + 3, 0, Math.PI * 2);
				ctx.strokeStyle = accent;
				ctx.lineWidth = 1.5;
				ctx.globalAlpha = 0.4 + pulse * 0.3;
				ctx.stroke();
				ctx.globalAlpha = 1;
			}

			// Label — only for connected/active/hovered nodes
			if (isActive || isHovered || hasLinks) {
				ctx.font = `${isActive ? 'bold 13' : isHovered ? '12' : '10'}px -apple-system, BlinkMacSystemFont, sans-serif`;
				ctx.fillStyle = isActive || isHovered ? textColor : textSecondary;
				ctx.textAlign = 'center';
				ctx.fillText(node.title, node.x, node.y - radius - 6);
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

	// Touch support for mobile
	let lastTouchDist = 0;

	function handleTouchStart(e: TouchEvent) {
		if (e.touches.length === 1) {
			const t = e.touches[0];
			mouseDownPos = { x: t.clientX, y: t.clientY };
			dragMoved = false;
			const node = getNodeAt(t.clientX, t.clientY);
			if (node) {
				dragging = node;
			} else {
				panning = true;
				panStart = { x: t.clientX - pan.x, y: t.clientY - pan.y };
			}
		} else if (e.touches.length === 2) {
			dragging = null;
			panning = false;
			const dx = e.touches[0].clientX - e.touches[1].clientX;
			const dy = e.touches[0].clientY - e.touches[1].clientY;
			lastTouchDist = Math.sqrt(dx * dx + dy * dy);
		}
	}

	function handleTouchMove(e: TouchEvent) {
		e.preventDefault();
		if (e.touches.length === 1) {
			const t = e.touches[0];
			const dx = t.clientX - mouseDownPos.x;
			const dy = t.clientY - mouseDownPos.y;
			if (Math.abs(dx) > 3 || Math.abs(dy) > 3) dragMoved = true;

			if (dragging && dragMoved) {
				const rect = canvas.getBoundingClientRect();
				dragging.x = (t.clientX - rect.left - pan.x) / zoom;
				dragging.y = (t.clientY - rect.top - pan.y) / zoom;
				dragging.vx = 0;
				dragging.vy = 0;
				draw();
			} else if (panning) {
				pan.x = t.clientX - panStart.x;
				pan.y = t.clientY - panStart.y;
				draw();
			}
		} else if (e.touches.length === 2) {
			const dx = e.touches[0].clientX - e.touches[1].clientX;
			const dy = e.touches[0].clientY - e.touches[1].clientY;
			const dist = Math.sqrt(dx * dx + dy * dy);
			if (lastTouchDist > 0) {
				const midX = (e.touches[0].clientX + e.touches[1].clientX) / 2;
				const midY = (e.touches[0].clientY + e.touches[1].clientY) / 2;
				const rect = canvas.getBoundingClientRect();
				const mx = midX - rect.left;
				const my = midY - rect.top;
				const oldZoom = zoom;
				zoom = Math.max(0.2, Math.min(5, zoom * (dist / lastTouchDist)));
				pan.x = mx - (mx - pan.x) * (zoom / oldZoom);
				pan.y = my - (my - pan.y) * (zoom / oldZoom);
				draw();
			}
			lastTouchDist = dist;
		}
	}

	function handleTouchEnd(e: TouchEvent) {
		if (e.touches.length === 0) {
			if (dragging && !dragMoved) {
				const node = dragging;
				dragging = null;
				onnavigate(node.path, node.title);
				return;
			}
			dragging = null;
			panning = false;
			lastTouchDist = 0;
		}
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
		if (glowFrame) cancelAnimationFrame(glowFrame);
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
				ontouchstart={handleTouchStart}
				ontouchmove={handleTouchMove}
				ontouchend={handleTouchEnd}
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

	@media (max-width: 768px) {
		.graph-panel {
			width: 100vw;
			height: 100vh;
			max-width: none;
			max-height: none;
			border-radius: 0;
			border: none;
		}
		.graph-header {
			padding-top: calc(env(safe-area-inset-top, 36px) + 14px);
		}
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
		touch-action: none;
	}

	.graph-canvas:active {
		cursor: grabbing;
	}

	.graph-loading {
		position: absolute;
		inset: 0;
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 10px;
		font-size: 13px;
		color: var(--text-tertiary);
		z-index: 1;
	}

	.spinner {
		animation: spin 1s linear infinite;
	}

	@keyframes spin {
		to { transform: rotate(360deg); }
	}
</style>

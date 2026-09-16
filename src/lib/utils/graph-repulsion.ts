export interface Body { x: number; y: number; vx: number; vy: number; }

const REPULSION = 1500;
const CUTOFF_SQ = 360000;
// Direct pairwise repulsion is exact and cheap for small graphs. Above this it is O(n²) per
// pass (50M pairs at 10,000 notes, #126), so repulsion switches to a Barnes-Hut quadtree.
export const DIRECT_LIMIT = 500;
const THETA_SQ = 0.81;

interface Quad { x0: number; y0: number; size: number; mass: number; cx: number; cy: number; bodies: Body[]; kids: (Quad | null)[] | null; }

const quad = (x0: number, y0: number, size: number): Quad => ({ x0, y0, size, mass: 0, cx: 0, cy: 0, bodies: [], kids: null });

function insert(q: Quad, n: Body) {
	q.cx = (q.cx * q.mass + n.x) / (q.mass + 1);
	q.cy = (q.cy * q.mass + n.y) / (q.mass + 1);
	q.mass += 1;
	// A leaf holds its bodies directly. Coincident bodies would split forever, so below a
	// pixel they share one leaf; each still repels exactly.
	if (!q.kids && (q.mass === 1 || q.size < 1)) { q.bodies.push(n); return; }
	if (!q.kids) {
		q.kids = [null, null, null, null];
		for (const previous of q.bodies) insertChild(q, previous);
		q.bodies = [];
	}
	insertChild(q, n);
}

function insertChild(q: Quad, n: Body) {
	const half = q.size / 2;
	const right = n.x >= q.x0 + half ? 1 : 0;
	const bottom = n.y >= q.y0 + half ? 1 : 0;
	const k = right + bottom * 2;
	q.kids![k] ??= quad(q.x0 + right * half, q.y0 + bottom * half, half);
	insert(q.kids![k]!, n);
}

function repel(q: Quad, n: Body) {
	const dx = q.cx - n.x, dy = q.cy - n.y;
	const distSq = dx * dx + dy * dy;
	// A cell containing n is always opened, so a body never repels itself.
	const containsNode = n.x >= q.x0 && n.x < q.x0 + q.size && n.y >= q.y0 && n.y < q.y0 + q.size;
	if (q.kids && (containsNode || q.size * q.size >= THETA_SQ * distSq)) {
		for (const kid of q.kids) if (kid) repel(kid, n);
		return;
	}
	if (q.kids) {
		pull(n, dx, dy, distSq, q.mass);
		return;
	}
	for (const b of q.bodies) {
		if (b === n) continue;
		const bx = b.x - n.x, by = b.y - n.y;
		pull(n, bx, by, bx * bx + by * by, 1);
	}
}

function pull(n: Body, dx: number, dy: number, distSq: number, mass: number) {
	if (distSq > CUTOFF_SQ) return;
	const d = distSq || 1;
	const dist = Math.sqrt(d);
	const force = (REPULSION * mass) / d;
	n.vx -= (dx / dist) * force;
	n.vy -= (dy / dist) * force;
}

/** Adds each body's repulsion from every other body to its velocity. */
export function applyRepulsion(bodies: Body[]) {
	const count = bodies.length;
	if (count <= DIRECT_LIMIT) {
		for (let i = 0; i < count; i++) {
			const a = bodies[i];
			for (let j = i + 1; j < count; j++) {
				const b = bodies[j];
				const dx = b.x - a.x, dy = b.y - a.y;
				const distSq = dx * dx + dy * dy;
				if (distSq > CUTOFF_SQ) continue;
				const d = distSq || 1;
				const force = REPULSION / d;
				const dist = Math.sqrt(d);
				const fx = (dx / dist) * force, fy = (dy / dist) * force;
				a.vx -= fx; a.vy -= fy; b.vx += fx; b.vy += fy;
			}
		}
		return;
	}
	let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
	for (const n of bodies) {
		if (n.x < minX) minX = n.x; if (n.y < minY) minY = n.y;
		if (n.x > maxX) maxX = n.x; if (n.y > maxY) maxY = n.y;
	}
	const root = quad(minX, minY, Math.max(maxX - minX, maxY - minY, 1) + 1);
	for (const n of bodies) insert(root, n);
	for (const n of bodies) repel(root, n);
}

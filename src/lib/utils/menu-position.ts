export type MenuViewport = { width: number; height: number };

export function clampMenuPosition(
	x: number,
	y: number,
	width: number,
	height: number,
	viewport: MenuViewport,
	margin = 8,
) {
	return {
		x: Math.max(margin, Math.min(x, Math.max(margin, viewport.width - width - margin))),
		y: Math.max(margin, Math.min(y, Math.max(margin, viewport.height - height - margin))),
	};
}

export function placeSubmenu(
	trigger: { left: number; right: number; top: number },
	width: number,
	height: number,
	viewport: MenuViewport,
	margin = 8,
) {
	const gap = 2;
	const right = trigger.right + gap;
	const left = trigger.left - width - gap;
	const flipLeft = right + width > viewport.width - margin && left >= margin;
	const { y } = clampMenuPosition(0, trigger.top - 4, width, height, viewport, margin);
	return {
		x: clampMenuPosition(flipLeft ? left : right, y, width, height, viewport, margin).x,
		y,
	};
}

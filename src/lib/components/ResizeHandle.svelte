<script lang="ts">
	let { onResize }: { onResize: (delta: number) => void } = $props();
	let active = $state(false);
	let startX = 0;

	function onMouseDown(e: MouseEvent) {
		e.preventDefault();
		active = true;
		startX = e.clientX;
		window.addEventListener('mousemove', onMouseMove);
		window.addEventListener('mouseup', onMouseUp);
	}

	function onMouseMove(e: MouseEvent) {
		const delta = e.clientX - startX;
		startX = e.clientX;
		onResize(delta);
	}

	function onMouseUp() {
		active = false;
		window.removeEventListener('mousemove', onMouseMove);
		window.removeEventListener('mouseup', onMouseUp);
	}
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="resize-handle" class:active onmousedown={onMouseDown}></div>

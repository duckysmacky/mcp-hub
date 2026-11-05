<script lang="ts">
	import type { Snippet } from 'svelte';

	let {
		id,
		value = $bindable(''),
		children
	}: { id: string; value?: string; children: Snippet } = $props();
</script>

<div class="cb-select-wrap">
	<select class="cb-select" {id} bind:value>
		{@render children()}
	</select>
	<svg class="cb-select-chev" width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
		<path d="M2 4l4 4 4-4" fill="none" stroke="currentColor" stroke-width="1.5" />
	</svg>
</div>

<script lang="ts">
	import type { Snippet } from 'svelte';

	const MARKS = { info: '[ i ]', warn: '[ ! ]', danger: '[ x ]', ok: '[ ok ]' };

	let {
		tone,
		tag,
		action,
		children
	}: {
		tone: keyof typeof MARKS;
		tag: string;
		action?: Snippet;
		children: Snippet;
	} = $props();
</script>

<div class={['cb-banner', `cb-banner--${tone}`]} role={tone === 'danger' ? 'alert' : 'status'}>
	<span class="cb-banner-tag">{MARKS[tone]} {tag}</span>
	<p class="cb-banner-body">{@render children()}</p>
	{@render action?.()}
</div>

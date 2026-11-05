<script lang="ts">
	import type { HTMLButtonAttributes } from 'svelte/elements';

	type Props = HTMLButtonAttributes & {
		variant?: 'primary' | 'secondary' | 'danger' | 'ghost' | 'ghost-danger';
		size?: 'sm' | 'lg';
		block?: boolean;
		busy?: boolean;
	};

	let {
		variant = 'secondary',
		size,
		block = false,
		busy = false,
		type = 'button',
		class: extraClass,
		onclick,
		children,
		...attributes
	}: Props = $props();

	const variantClasses = $derived(
		variant === 'ghost-danger' ? 'cb-btn--ghost cb-btn--ghost-danger' : `cb-btn--${variant}`
	);
</script>

<!-- Stays enabled while busy, so keyboard focus isn't lost: the click is just ignored. -->
<button
	{...attributes}
	{type}
	class={[
		'cb-btn',
		variantClasses,
		size && `cb-btn--${size}`,
		block && 'cb-btn--block',
		extraClass
	]}
	aria-busy={busy || undefined}
	onclick={(event) => {
		if (busy) {
			event.preventDefault();
			return;
		}
		onclick?.(event);
	}}
>
	{#if busy}<span class="cb-spinner" aria-hidden="true"></span>{/if}
	{@render children?.()}
</button>

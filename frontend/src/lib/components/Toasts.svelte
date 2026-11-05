<script lang="ts">
	import Button from '#lib/components/Button.svelte';
	import { toasts } from '#lib/toasts.svelte.ts';

	const MARKS = { ok: '[ ok ]', info: '[ i ]', err: '[ x ]' };
</script>

<div class="cb-toasts">
	{#each toasts.items as toast (toast.id)}
		<div
			class={['cb-toast', `cb-toast--${toast.tone}`]}
			role={toast.tone === 'err' ? 'alert' : 'status'}
			onmouseenter={() => toasts.pause(toast.id)}
			onmouseleave={() => toasts.resume(toast)}
			onfocusin={() => toasts.pause(toast.id)}
			onfocusout={() => toasts.resume(toast)}
		>
			<span class="cb-toast-tag">{MARKS[toast.tone]}</span>
			<span class="cb-toast-body">{toast.message}</span>
			{#if toast.action}
				{@const action = toast.action}
				<Button
					variant="ghost"
					size="sm"
					onclick={() => {
						toasts.dismiss(toast.id);
						action.run();
					}}
				>
					{action.label}
				</Button>
			{:else if toast.tone === 'err'}
				<Button variant="ghost" size="sm" onclick={() => toasts.dismiss(toast.id)}>Dismiss</Button>
			{/if}
		</div>
	{/each}
</div>

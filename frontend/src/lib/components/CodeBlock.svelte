<script lang="ts">
	import { toasts } from '#lib/toasts.svelte.ts';

	let { value, name }: { value: string; name: string } = $props();

	const COPIED_FEEDBACK_MS = 2000;

	// The part that is the user's own stands out.
	const pathStart = $derived(value.lastIndexOf('/mcp/'));
	const host = $derived(pathStart === -1 ? value : value.slice(0, pathStart));
	const path = $derived(pathStart === -1 ? '' : value.slice(pathStart));

	let copied = $state(false);
	let resetTimer: ReturnType<typeof setTimeout> | undefined;

	async function copy() {
		try {
			await navigator.clipboard.writeText(value);
		} catch {
			toasts.show('err', 'Could not reach the clipboard. Select the text and copy it yourself.');
			return;
		}
		copied = true;
		toasts.show('ok', `Copied ${name}`);
		clearTimeout(resetTimer);
		resetTimer = setTimeout(() => (copied = false), COPIED_FEEDBACK_MS);
	}
</script>

<div class="cb-code">
	<code><span class="cb-code-host">{host}</span><span class="cb-code-path">{path}</span></code>
	<button
		class={['cb-btn cb-btn--ghost', copied && 'is-copied']}
		type="button"
		aria-label={`Copy ${name}`}
		onclick={copy}
	>
		{copied ? 'Copied' : 'Copy'}
	</button>
</div>

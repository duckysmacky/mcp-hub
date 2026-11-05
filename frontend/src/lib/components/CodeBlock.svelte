<script lang="ts">
	import { i18n } from '#lib/i18n/index.svelte.ts';
	import { toasts } from '#lib/toasts.svelte.ts';

	let { value, name }: { value: string; name: string } = $props();

	const COPIED_FEEDBACK_MS = 2000;

	const isMultiline = $derived(value.includes('\n'));
	// In a URL or a command ending in one, the part that is the user's own stands out.
	const pathStart = $derived(isMultiline ? -1 : value.lastIndexOf('/mcp/'));

	let copied = $state(false);
	let resetTimer: ReturnType<typeof setTimeout> | undefined;

	async function copy() {
		try {
			await navigator.clipboard.writeText(value);
		} catch {
			toasts.show('err', i18n.t.common.clipboardFailed);
			return;
		}
		copied = true;
		toasts.show('ok', i18n.t.common.copiedName(name));
		clearTimeout(resetTimer);
		resetTimer = setTimeout(() => (copied = false), COPIED_FEEDBACK_MS);
	}
</script>

<div class="cb-code">
	<!-- No whitespace inside: the block has to copy and display exactly this value. -->
	<code class={{ multiline: isMultiline }}
		>{#if pathStart === -1}{value}{:else}<span class="cb-code-host"
				>{value.slice(0, pathStart)}</span
			><span class="cb-code-path">{value.slice(pathStart)}</span>{/if}</code
	>
	<button
		class={['cb-btn cb-btn--ghost', copied && 'is-copied']}
		type="button"
		aria-label={i18n.t.common.copyName(name)}
		onclick={copy}
	>
		{copied ? i18n.t.common.copied : i18n.t.common.copy}
	</button>
</div>

<style>
	code.multiline {
		display: block;
		padding-block: var(--space-3);
		white-space: pre;
		font-size: 12px;
		line-height: 18px;
	}
</style>

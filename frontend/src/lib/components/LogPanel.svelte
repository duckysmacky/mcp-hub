<script lang="ts">
	let { output }: { output: string } = $props();

	const STICK_THRESHOLD_PX = 4;

	const lines = $derived(output.trimEnd().split('\n'));

	let stuckToBottom = true;

	function trackScroll(event: { currentTarget: HTMLElement }) {
		const log = event.currentTarget;
		const fromBottom = log.scrollHeight - log.scrollTop - log.clientHeight;
		stuckToBottom = fromBottom <= STICK_THRESHOLD_PX;
	}
</script>

<section class="cb-panel">
	<div class="cb-log-head">
		<h2 class="cb-label">Output</h2>
		<span class="cb-hint">redacted</span>
	</div>
	<!-- Focusable so keyboard users can scroll it. -->
	<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
	<div
		class="cb-log"
		onscroll={trackScroll}
		{@attach (log) => {
			// Follows new output, unless the user scrolled up to read something.
			if (lines && stuckToBottom) {
				log.scrollTop = log.scrollHeight;
			}
		}}
		tabindex="0"
		role="log"
		aria-label="Server output"
	>
		{#if output.trim()}
			{#each lines as line, index (index)}
				<div class="cb-log-line">{line}</div>
			{/each}
		{:else}
			<div class="cb-log-empty">No output yet. It appears here once an app connects.</div>
		{/if}
	</div>
</section>

<style>
	.cb-log-head {
		border-top: 0;
	}
</style>

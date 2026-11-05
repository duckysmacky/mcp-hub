<script lang="ts">
	let { output }: { output: string } = $props();

	const STICK_THRESHOLD_PX = 4;

	const lines = $derived(output.trimEnd().split('\n'));

	let log = $state<HTMLElement>();
	let stuckToBottom = true;

	// Follows new output, unless the user scrolled up to read something.
	$effect(() => {
		if (log && lines && stuckToBottom) {
			log.scrollTop = log.scrollHeight;
		}
	});

	function trackScroll() {
		if (log) {
			const fromBottom = log.scrollHeight - log.scrollTop - log.clientHeight;
			stuckToBottom = fromBottom <= STICK_THRESHOLD_PX;
		}
	}
</script>

<section class="cb-panel">
	<div class="cb-log-head">
		<h2 class="cb-label">Output</h2>
		<span class="cb-hint">redacted</span>
	</div>
	<!-- svelte-ignore a11y_no_noninteractive_tabindex: keyboard users have to be able to scroll it -->
	<div
		class="cb-log"
		bind:this={log}
		onscroll={trackScroll}
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

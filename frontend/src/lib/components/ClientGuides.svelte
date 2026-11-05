<script lang="ts">
	import { clientGuides } from '#lib/clients.ts';
	import CodeBlock from '#lib/components/CodeBlock.svelte';

	let { slug, url }: { slug: string; url: string } = $props();

	const guides = $derived(clientGuides(slug, url));
</script>

{#snippet chevron()}
	<svg class="chevron" width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
		<path d="M2 4l4 4 4-4" fill="none" stroke="currentColor" stroke-width="1.5" />
	</svg>
{/snippet}

<details class="section">
	<summary class="cb-row">
		<span class="cb-label">Connect an app</span>
		{@render chevron()}
	</summary>
	<div class="cb-rows">
		{#each guides as guide (guide.name)}
			<details>
				<summary class="cb-row">
					<strong>{guide.name}</strong>
					{@render chevron()}
				</summary>
				<ol class="cb-steps">
					{#each guide.steps as step, index (step.text)}
						<li>
							<span>{index + 1}</span>
							<div class="step">
								<span>{step.text}</span>
								{#if step.copy}
									<CodeBlock value={step.copy} name={`${guide.name} step ${index + 1}`} />
								{/if}
							</div>
						</li>
					{/each}
				</ol>
			</details>
		{/each}
	</div>
</details>

<style>
	.section {
		border-top: var(--bw) solid var(--border);
	}
	.section > .cb-rows {
		border-top: var(--bw) solid var(--border);
	}
	summary {
		justify-content: space-between;
		flex-wrap: nowrap;
		cursor: pointer;
		list-style: none;
	}
	summary::-webkit-details-marker {
		display: none;
	}
	summary:hover {
		background: var(--surface-2);
	}
	.chevron {
		flex: none;
		color: var(--text-2);
	}
	details[open] > summary > .chevron {
		transform: rotate(180deg);
	}
	.cb-steps {
		padding: var(--space-3) var(--space-5) var(--space-5);
		gap: var(--space-3);
	}
	.step {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
	}
	@media (max-width: 639px) {
		.cb-steps {
			padding-left: var(--space-4);
			padding-right: var(--space-4);
		}
	}
</style>

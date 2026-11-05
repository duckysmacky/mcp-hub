<script lang="ts">
	import { api, messageOf } from '#lib/api/client.ts';
	import type { ToolStatus } from '#lib/api/types.ts';
	import { i18n } from '#lib/i18n/index.svelte.ts';
	import { toasts } from '#lib/toasts.svelte.ts';

	let { instanceId, tools: loaded }: { instanceId: number; tools: ToolStatus[] } = $props();

	const SEARCH_THRESHOLD = 10;

	const t = $derived(i18n.t.tools);
	// Flips right away on click; the parent replaces it only when reloading.
	let tools = $derived(loaded);
	let search = $state('');

	const enabledCount = $derived(tools.filter((tool) => !tool.disabled).length);
	const visible = $derived.by(() => {
		const query = search.trim().toLowerCase();
		return tools.filter(
			(tool) =>
				tool.name.toLowerCase().includes(query) || tool.description?.toLowerCase().includes(query)
		);
	});

	// One at a time, so rapid clicks can't land out of order and leave an older list saved.
	let saving: Promise<unknown> = Promise.resolve();

	function toggle(name: string) {
		tools = tools.map((tool) =>
			tool.name === name ? { ...tool, disabled: !tool.disabled } : tool
		);
		const disabledTools = tools.filter((tool) => tool.disabled).map((tool) => tool.name);
		saving = saving.then(async () => {
			try {
				await api.patch(`/instances/${instanceId}`, { disabled_tools: disabledTools });
			} catch (failure) {
				toasts.show('err', messageOf(failure));
				tools = await api.get<ToolStatus[]>(`/instances/${instanceId}/tools`);
			}
		});
	}
</script>

<section class="cb-panel">
	<div class="cb-panel-head">
		<h2 class="cb-label">{t.heading}</h2>
		{#if tools.length > 0}
			<span class="cb-hint">{t.enabledCount(enabledCount, tools.length)}</span>
		{/if}
	</div>

	{#if tools.length === 0}
		<p class="cb-panel-body cb-field-help">{t.empty}</p>
	{:else}
		{#if tools.length > SEARCH_THRESHOLD}
			<div class="cb-panel-body search">
				<label class="cb-label" for="tool-search">{t.search}</label>
				<input
					class="cb-input"
					id="tool-search"
					type="search"
					bind:value={search}
					autocomplete="off"
					autocapitalize="none"
					spellcheck="false"
				/>
			</div>
		{/if}
		<div class="cb-rows list">
			{#each visible as tool (tool.name)}
				<div class="cb-row">
					<div class="tool">
						<strong>{tool.name}</strong>
						{#if tool.description}<div class="cb-hint description">{tool.description}</div>{/if}
					</div>
					<button
						class="cb-toggle"
						type="button"
						role="switch"
						aria-checked={!tool.disabled}
						aria-label={tool.name}
						onclick={() => toggle(tool.name)}
					>
						<span class="cb-toggle-track"><span class="cb-toggle-thumb"></span></span>
						<span class="cb-toggle-text">{tool.disabled ? t.off : t.on}</span>
					</button>
				</div>
			{:else}
				<div class="cb-row cb-muted">{t.noMatch}</div>
			{/each}
		</div>
		<p class="cb-panel-foot cb-field-help">{t.help}</p>
	{/if}
</section>

<style>
	.search {
		display: flex;
		flex-direction: column;
		gap: 6px;
		border-bottom: var(--bw) solid var(--border);
	}
	/* Some servers list a hundred tools: this panel scrolls instead of the page. */
	.list {
		max-height: 480px;
		overflow-y: auto;
	}
	.cb-row {
		flex-wrap: nowrap;
	}
	.tool {
		flex: 1;
		min-width: 0;
		overflow-wrap: anywhere;
	}
	.description {
		display: -webkit-box;
		-webkit-box-orient: vertical;
		-webkit-line-clamp: 2;
		line-clamp: 2;
		overflow: hidden;
	}
	.cb-toggle-text {
		width: 4ch;
	}
</style>

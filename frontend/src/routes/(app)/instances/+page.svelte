<script lang="ts">
	import StatusBadge from '#lib/components/StatusBadge.svelte';
	import { i18n } from '#lib/i18n/index.svelte.ts';
	import { statusLabel } from '#lib/status.ts';
	import { resolve } from '$app/paths';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
</script>

<svelte:head>
	<title>{i18n.t.instances.title} - MCP Hub</title>
</svelte:head>

<div class="cb-page-head">
	<h1 class="cb-page-title">{i18n.t.instances.title}</h1>
	{#if data.rows.length > 0}
		<a class="cb-btn cb-btn--primary" href={resolve('/(app)/instances/new')}
			>{i18n.t.instances.newInstance}</a
		>
	{/if}
</div>

{#if data.rows.length === 0}
	<div class="cb-empty">
		<span class="cb-empty-mark">{i18n.t.instances.emptyMark}</span>
		<h2 class="cb-empty-title">{i18n.t.instances.emptyTitle}</h2>
		<p class="cb-empty-body">
			{i18n.t.instances.emptyBody}
		</p>
		<a class="cb-btn cb-btn--primary" href={resolve('/(app)/instances/new')}
			>{i18n.t.instances.newInstance}</a
		>
	</div>
{:else}
	<section class="cb-panel">
		<div class="cb-rows">
			{#each data.rows as { instance, status } (instance.id)}
				<a
					class={['cb-row cb-row--link', status.needs_attention && 'cb-row--warn']}
					href={resolve('/(app)/instances/[id]', { id: String(instance.id) })}
				>
					<strong class="slug">{instance.slug}</strong>
					<span class="server">{instance.server_id}</span>
					<span class="state"><StatusBadge status={statusLabel(status)} /></span>
				</a>
			{/each}
		</div>
	</section>
{/if}

<style>
	.slug {
		flex: 1 1 200px;
		min-width: 0;
		overflow-wrap: anywhere;
	}
	.server {
		flex: 1 1 160px;
		font-size: 12px;
		color: var(--text-2);
	}
	/* Wide enough for the longest badge, so the server column lines up between rows. */
	.state {
		flex: 0 0 160px;
		display: flex;
		justify-content: flex-end;
	}
	@media (max-width: 639px) {
		.state {
			flex-basis: 100%;
			justify-content: flex-start;
		}
	}
</style>

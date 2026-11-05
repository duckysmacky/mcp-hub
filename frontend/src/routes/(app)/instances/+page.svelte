<script lang="ts">
	import StatusBadge from '#lib/components/StatusBadge.svelte';
	import { statusLabel } from '#lib/status.ts';
	import { resolve } from '$app/paths';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
</script>

<svelte:head>
	<title>Instances - MCP Hub</title>
</svelte:head>

<div class="cb-page-head">
	<h1 class="cb-page-title">Instances</h1>
	{#if data.rows.length > 0}
		<a class="cb-btn cb-btn--primary" href={resolve('/(app)/instances/new')}>New instance</a>
	{/if}
</div>

{#if data.rows.length === 0}
	<div class="cb-empty">
		<span class="cb-empty-mark">[ 0 instances ]</span>
		<h2 class="cb-empty-title">No servers yet</h2>
		<p class="cb-empty-body">
			Pick a server from the catalog, give it a slug and add your credentials. You get a personal
			URL to paste into Claude.
		</p>
		<a class="cb-btn cb-btn--primary" href={resolve('/(app)/instances/new')}>New instance</a>
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

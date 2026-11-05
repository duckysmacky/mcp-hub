<script lang="ts">
	import { api, messageOf } from '#lib/api/client.ts';
	import { RUNTIMES, type Instance, type Runtime } from '#lib/api/types.ts';
	import Button from '#lib/components/Button.svelte';
	import SegmentedControl from '#lib/components/SegmentedControl.svelte';
	import Select from '#lib/components/Select.svelte';
	import SlugField from '#lib/components/SlugField.svelte';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	let serverId = $derived(data.servers[0]?.id ?? '');
	const server = $derived(data.servers.find((candidate) => candidate.id === serverId));
	// These follow the chosen server until the user edits them.
	let slug = $derived(data.requestedSlug ?? serverId);
	let runtime = $derived<Runtime>(server?.default_runtime ?? 'bun');

	let error = $state('');
	let busy = $state(false);

	async function create(event: SubmitEvent) {
		event.preventDefault();
		busy = true;
		error = '';
		try {
			const instance = await api.post<Instance>('/instances', {
				server_id: serverId,
				slug,
				runtime
			});
			await goto(resolve('/(app)/instances/[id]', { id: String(instance.id) }));
		} catch (failure) {
			error = messageOf(failure);
		} finally {
			busy = false;
		}
	}
</script>

<svelte:head>
	<title>New instance - MCP Hub</title>
</svelte:head>

<div class="cb-crumbs">
	<a href={resolve('/(app)/instances')}>instances</a> / <span class="crumb-here">new</span>
</div>

<div class="cb-page-head">
	<h1 class="cb-page-title">New instance</h1>
</div>

{#if !server}
	<div class="cb-empty">
		<span class="cb-empty-mark">[ 0 servers ]</span>
		<h2 class="cb-empty-title">The catalog is empty</h2>
		<p class="cb-empty-body">
			Only servers in the operator's catalog can run here, and this hub has none yet. Add one to
			catalog.toml and restart the hub.
		</p>
	</div>
{:else}
	<div class="cb-cols">
		<form class="cb-panel cb-col-main" onsubmit={create}>
			<div class="fields">
				<div class="cb-field">
					<label class="cb-label" for="server">Server</label>
					<Select id="server" bind:value={serverId}>
						{#each data.servers as option (option.id)}
							<option value={option.id}>{option.id}</option>
						{/each}
					</Select>
				</div>
				<SlugField id="slug" bind:value={slug} {error} />
				<div class="cb-field">
					<span class="cb-label">Runtime</span>
					<div>
						<SegmentedControl
							label="Runtime"
							name="runtime"
							options={RUNTIMES}
							bind:value={runtime}
						/>
					</div>
				</div>
			</div>
			<div class="cb-panel-foot">
				<Button type="submit" variant="primary" {busy}>
					{busy ? 'Creating instance' : 'Create instance'}
				</Button>
				<a class="cb-btn cb-btn--ghost" href={resolve('/(app)/instances')}>Cancel</a>
			</div>
		</form>

		<section class="cb-panel cb-col-side">
			<div class="cb-panel-head">
				<h2 class="cb-label">Server</h2>
				<span class="cb-hint">{server.id}</span>
			</div>
			<dl class="cb-kv-list">
				<div class="cb-kv">
					<dt>Mode</dt>
					<dd>{server.mode}</dd>
				</div>
				<div class="cb-kv">
					<dt>Default</dt>
					<dd>{server.default_runtime}</dd>
				</div>
				<div class="cb-kv">
					<dt>Variables</dt>
					<dd>
						{#each server.env_vars as name (name)}
							<div>{name}</div>
						{:else}
							<span class="cb-muted">None</span>
						{/each}
					</dd>
				</div>
			</dl>
		</section>
	</div>
{/if}

<style>
	.fields {
		display: flex;
		flex-direction: column;
		gap: var(--space-5);
		padding: var(--space-5);
	}
	.crumb-here {
		color: var(--text);
	}
</style>

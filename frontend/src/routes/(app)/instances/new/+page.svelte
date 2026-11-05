<script lang="ts">
	import { api, messageOf } from '#lib/api/client.ts';
	import type { Instance } from '#lib/api/types.ts';
	import Banner from '#lib/components/Banner.svelte';
	import Button from '#lib/components/Button.svelte';
	import CustomServerForm from '#lib/components/CustomServerForm.svelte';
	import Select from '#lib/components/Select.svelte';
	import SlugField from '#lib/components/SlugField.svelte';
	import { definitionOf, emptyDraft } from '#lib/custom-server.ts';
	import { i18n } from '#lib/i18n/index.svelte.ts';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	const CUSTOM = 'custom';

	let serverId = $derived(data.servers[0]?.id ?? (data.settings.custom_servers ? CUSTOM : ''));
	const isCustom = $derived(serverId === CUSTOM);
	let draft = $state(emptyDraft());
	const server = $derived(data.servers.find((candidate) => candidate.id === serverId));
	// Follows the chosen server until the user edits it.
	let slug = $derived(data.requestedSlug ?? serverId);

	let error = $state('');
	let busy = $state(false);

	async function create(event: SubmitEvent) {
		event.preventDefault();
		busy = true;
		error = '';
		try {
			const body = isCustom
				? { slug, custom: definitionOf($state.snapshot(draft)) }
				: { slug, server_id: serverId };
			const instance = await api.post<Instance>('/instances', body);
			await goto(resolve('/(app)/instances/[id]', { id: String(instance.id) }));
		} catch (failure) {
			error = messageOf(failure);
		} finally {
			busy = false;
		}
	}
</script>

<svelte:head>
	<title>{i18n.t.instances.newInstance} - MCP Hub</title>
</svelte:head>

<div class="cb-crumbs">
	<a href={resolve('/(app)/instances')}>{i18n.t.instances.crumb}</a> /
	<span class="crumb-here">{i18n.t.newInstance.crumb}</span>
</div>

<div class="cb-page-head">
	<h1 class="cb-page-title">{i18n.t.instances.newInstance}</h1>
</div>

{#if !server && !isCustom}
	<div class="cb-empty">
		<span class="cb-empty-mark">{i18n.t.newInstance.emptyMark}</span>
		<h2 class="cb-empty-title">{i18n.t.newInstance.emptyTitle}</h2>
		<p class="cb-empty-body">{i18n.t.newInstance.emptyBody}</p>
	</div>
{:else}
	{#if isCustom}
		<Banner tone="warn" tag={i18n.t.custom.warningTag}>
			<strong>{i18n.t.custom.warningLead}</strong>
			{i18n.t.custom.warningBody}
		</Banner>
	{/if}
	<div class="cb-cols">
		<form class="cb-panel cb-col-main" onsubmit={create}>
			<div class="fields">
				<div class="cb-field">
					<label class="cb-label" for="server">{i18n.t.newInstance.server}</label>
					<Select id="server" bind:value={serverId}>
						{#each data.servers as option (option.id)}
							<option value={option.id}>{option.id}</option>
						{/each}
						{#if data.settings.custom_servers}
							<option value={CUSTOM}>{i18n.t.custom.option}</option>
						{/if}
					</Select>
				</div>
				<SlugField id="slug" bind:value={slug} error={isCustom ? '' : error} />
				{#if isCustom}
					<CustomServerForm idPrefix="custom" bind:draft />
					{#if error}
						<p class="cb-field-error" role="alert">{error}</p>
					{/if}
				{/if}
			</div>
			<div class="cb-panel-foot">
				<Button type="submit" variant="primary" {busy}>
					{busy ? i18n.t.newInstance.creating : i18n.t.newInstance.create}
				</Button>
				<a class="cb-btn cb-btn--ghost" href={resolve('/(app)/instances')}>
					{i18n.t.newInstance.cancel}
				</a>
			</div>
		</form>

		{#if server && !isCustom}
			<section class="cb-panel cb-col-side">
				<div class="cb-panel-head">
					<h2 class="cb-label">{i18n.t.newInstance.server}</h2>
					<span class="cb-hint">{server.id}</span>
				</div>
				<dl class="cb-kv-list">
					<div class="cb-kv">
						<dt>{i18n.t.instance.kvMode}</dt>
						<dd>{server.mode}</dd>
					</div>
					<div class="cb-kv">
						<dt>{i18n.t.instance.kvRuntime}</dt>
						<dd>{server.runtime}</dd>
					</div>
					<div class="cb-kv">
						<dt>{i18n.t.instance.kvPackage}</dt>
						<dd>{server.package}</dd>
					</div>
				</dl>
				<div class="cb-log-head">
					<h3 class="cb-label">{i18n.t.newInstance.variables}</h3>
				</div>
				<div class="cb-rows">
					{#each server.env as spec (spec.name)}
						<div class="cb-row variable">
							<div>
								<strong>{spec.name}</strong>
								{#if spec.required}<span class="cb-tag">{i18n.t.common.required}</span>{/if}
							</div>
							{#if spec.description}<span class="cb-hint">{spec.description}</span>{/if}
						</div>
					{:else}
						<div class="cb-row cb-muted">{i18n.t.newInstance.noVariables}</div>
					{/each}
				</div>
			</section>
		{/if}
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
	.variable {
		flex-direction: column;
		align-items: flex-start;
		overflow-wrap: anywhere;
	}
</style>

<script lang="ts">
	import { api, messageOf } from '#lib/api/client.ts';
	import type { EnvSpec, EnvStatus } from '#lib/api/types.ts';
	import Button from '#lib/components/Button.svelte';
	import ConfirmDialog from '#lib/components/ConfirmDialog.svelte';
	import EnvVarRow from '#lib/components/EnvVarRow.svelte';
	import { i18n } from '#lib/i18n/index.svelte.ts';
	import { toasts } from '#lib/toasts.svelte.ts';

	let {
		instanceId,
		slug,
		env,
		specs,
		missingRequired,
		liveProcesses,
		onchange,
		onrestart
	}: {
		instanceId: number;
		slug: string;
		env: EnvStatus[];
		specs: EnvSpec[];
		missingRequired: string[];
		liveProcesses: number;
		onchange: () => Promise<void>;
		onrestart: () => void;
	} = $props();

	const t = $derived(i18n.t.env);
	const entryOf = (name: string) => env.find((entry) => entry.var === name);
	const required = $derived(specs.filter((spec) => spec.required));
	const optional = $derived(specs.filter((spec) => !spec.required));
	const optionalSetCount = $derived(optional.filter((spec) => entryOf(spec.name)).length);
	// Set values the catalog can't describe: it dropped the variable, or the server is off.
	const orphans = $derived(env.filter((entry) => !specs.some((spec) => spec.name === entry.var)));

	let removing = $state<string | null>(null);

	const pathOf = (name: string) => `/instances/${instanceId}/env/${encodeURIComponent(name)}`;

	/** Throws on failure, so the row can show the error beside its own input. */
	async function save(name: string, value: string) {
		await api.put(pathOf(name), { value });
		await onchange();
		if (liveProcesses > 0) {
			toasts.show('info', t.savedRestart(name), { label: t.restart, run: onrestart });
		} else {
			toasts.show('ok', t.saved(name));
		}
	}

	async function remove(name: string) {
		removing = null;
		try {
			await api.delete(pathOf(name));
			await onchange();
			toasts.show('ok', t.removed(name));
		} catch (failure) {
			toasts.show('err', messageOf(failure));
		}
	}
</script>

{#snippet row(spec: EnvSpec)}
	<EnvVarRow
		{spec}
		entry={entryOf(spec.name)}
		savedValue={entryOf(spec.name)?.value ?? ''}
		conflicts={spec.conflicts.filter((name) => entryOf(name))}
		isMissing={missingRequired.includes(spec.name)}
		onsave={save}
		onremove={(name) => (removing = name)}
	/>
{/snippet}

<section class="cb-panel">
	<div class="cb-panel-head">
		<h2 class="cb-label">{t.heading}</h2>
		<span class="cb-hint">{t.hint}</span>
	</div>

	{#if required.length > 0}
		<div class="group-head">
			<h3 class="cb-label">{t.required}</h3>
		</div>
		<div class="cb-rows">
			{#each required as spec (spec.name)}
				{@render row(spec)}
			{/each}
		</div>
	{/if}

	{#if optional.length > 0}
		<details class={{ 'after-rows': required.length > 0 }} open>
			<summary class="group-head">
				<h3 class="cb-label">{t.optional}</h3>
				<span class="summary-end">
					<span class="cb-hint">{t.optionalCount(optionalSetCount, optional.length)}</span>
					<svg class="chevron" width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
						<path d="M2 4l4 4 4-4" fill="none" stroke="currentColor" stroke-width="1.5" />
					</svg>
				</span>
			</summary>
			<div class="cb-rows">
				{#each optional as spec (spec.name)}
					{@render row(spec)}
				{/each}
			</div>
		</details>
	{/if}

	{#if orphans.length > 0}
		<div class="cb-rows orphans">
			{#each orphans as entry (entry.var)}
				<div class="cb-row">
					{#if entry.in_catalog}
						<strong class="name">{entry.var}</strong>
						<span class="state">
							<span class="cb-state cb-state--ok">{t.stateSet}</span> -
							{entry.updated_at.slice(0, 10)}
							{#if entry.value !== undefined}<code class="current">{entry.value}</code>{/if}
						</span>
					{:else}
						<strong class="name stale"><s>{entry.var}</s></strong>
						<span class="state">
							<span class="cb-state cb-state--muted">{t.stateUnused}</span> -
							{t.setOn(entry.updated_at.slice(0, 10))}
						</span>
					{/if}
					<Button variant="ghost-danger" size="sm" onclick={() => (removing = entry.var)}>
						{t.remove}
					</Button>
				</div>
			{/each}
		</div>
	{/if}
</section>

{#if removing}
	{@const name = removing}
	<ConfirmDialog
		context={i18n.t.common.instanceContext(slug)}
		title={t.removeTitle(name)}
		detail={t.removeDetail}
		action={t.removeAction}
		onconfirm={() => remove(name)}
		oncancel={() => (removing = null)}
	/>
{/if}

<style>
	.group-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
		min-height: 36px;
		padding: 0 var(--space-5);
		background: var(--surface-2);
		border-bottom: var(--bw) solid var(--border);
	}
	.after-rows {
		border-top: var(--bw) solid var(--border);
	}
	details:not([open]) > .group-head {
		border-bottom: 0;
	}
	summary {
		cursor: pointer;
		list-style: none;
	}
	summary::-webkit-details-marker {
		display: none;
	}
	summary:hover {
		background: var(--surface-3);
	}
	.summary-end {
		display: flex;
		align-items: center;
		gap: var(--space-3);
	}
	.chevron {
		flex: none;
		color: var(--text-2);
	}
	details[open] .chevron {
		transform: rotate(180deg);
	}
	.orphans {
		border-top: var(--bw) solid var(--border-strong);
	}
	.name {
		flex: 1 1 240px;
		min-width: 0;
		overflow-wrap: anywhere;
	}
	.stale {
		color: var(--text-3);
		font-weight: 500;
	}
	.state {
		flex: 1 1 180px;
		min-width: 0;
		font-size: 12px;
		color: var(--text-2);
	}
	.current {
		display: block;
		color: var(--text);
		overflow-wrap: anywhere;
	}
	@media (max-width: 639px) {
		.group-head {
			min-height: var(--touch);
			padding: 0 var(--space-4);
		}
	}
</style>

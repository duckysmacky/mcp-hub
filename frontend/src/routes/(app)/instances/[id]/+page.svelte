<script lang="ts">
	import { api, messageOf } from '#lib/api/client.ts';
	import type { InstanceStatus, ToolStatus } from '#lib/api/types.ts';
	import Banner from '#lib/components/Banner.svelte';
	import Button from '#lib/components/Button.svelte';
	import ClientGuides from '#lib/components/ClientGuides.svelte';
	import CodeBlock from '#lib/components/CodeBlock.svelte';
	import ConfirmDialog from '#lib/components/ConfirmDialog.svelte';
	import EnvVars from '#lib/components/EnvVars.svelte';
	import LogPanel from '#lib/components/LogPanel.svelte';
	import SlugField from '#lib/components/SlugField.svelte';
	import StatusBadge from '#lib/components/StatusBadge.svelte';
	import { i18n } from '#lib/i18n/index.svelte.ts';
	import ToolFilter from '#lib/components/ToolFilter.svelte';
	import { failedExit, statusLabel } from '#lib/status.ts';
	import { toasts } from '#lib/toasts.svelte.ts';
	import { goto, invalidateAll } from '$app/navigation';
	import { resolve } from '$app/paths';
	import type { PageProps } from './$types';

	const STATUS_POLL_MS = 5000;

	type Action = 'start' | 'stop' | 'restart' | 'delete' | 'rename';
	type Confirmation = {
		name: Action;
		title: string;
		detail: string;
		action: string;
		variant?: 'danger' | 'secondary';
		run: () => Promise<void>;
	};

	let { data }: PageProps = $props();

	const t = $derived(i18n.t.instance);
	const instance = $derived(data.instance);
	const server = $derived(data.servers.find((candidate) => candidate.id === instance.server_id));
	const path = $derived(`/instances/${instance.id}`);

	let status = $derived<InstanceStatus>(data.status);
	const label = $derived(statusLabel(status, instance.missing_required));
	const lastFailure = $derived(failedExit(status));
	const cannotStart = $derived(
		instance.needs_attention === 'server_removed' || instance.needs_attention === 'server_disabled'
	);

	let tools = $derived<ToolStatus[]>(data.tools);
	let slug = $derived(instance.slug);
	let settingsError = $state('');

	let confirmation = $state.raw<Confirmation | null>(null);
	let pendingAction = $state<Action | null>(null);

	// Polls while this page is open and visible. Background tabs stay quiet.
	$effect(() => {
		const id = instance.id;
		const timer = setInterval(async () => {
			if (document.visibilityState !== 'visible') {
				return;
			}
			try {
				status = await api.get<InstanceStatus>(`/instances/${id}/status`);
				// Only while empty: once listed, the filter owns its own state.
				if (tools.length === 0) {
					tools = await api.get<ToolStatus[]>(`/instances/${id}/tools`);
				}
			} catch {
				// The next tick tries again; a lost session is already redirected by the client.
			}
		}, STATUS_POLL_MS);
		return () => clearInterval(timer);
	});

	async function act(name: Action, action: () => Promise<void>) {
		confirmation = null;
		pendingAction = name;
		try {
			await action();
			await invalidateAll();
		} catch (failure) {
			toasts.show('err', messageOf(failure));
		} finally {
			pendingAction = null;
		}
	}

	const liveNote = $derived(status.live_processes > 0 ? t.liveNote(status.live_processes) : '');

	const start = () => act('start', () => api.post(`${path}/start`));

	function askToStop() {
		confirmation = {
			name: 'stop',
			title: t.stopTitle,
			detail: `${liveNote}${t.stopDetail}`,
			action: t.stopAction,
			run: () => api.post(`${path}/stop`)
		};
	}

	function askToRestart() {
		confirmation = {
			name: 'restart',
			title: t.restartTitle,
			detail: `${liveNote}${t.restartDetail}`,
			action: t.restartAction,
			variant: 'secondary',
			run: async () => {
				await api.post(`${path}/restart`);
				toasts.show('ok', t.restarted(instance.slug));
			}
		};
	}

	function askToDelete() {
		confirmation = {
			name: 'delete',
			title: t.deleteTitle,
			detail: t.deleteDetail,
			action: t.deleteHeading,
			run: async () => {
				await api.delete(path);
				toasts.show('ok', t.deleted(instance.slug));
				await goto(resolve('/(app)/instances'));
			}
		};
	}

	async function rename() {
		settingsError = '';
		try {
			await api.patch(path, { slug });
			toasts.show('ok', t.settingsSaved);
		} catch (failure) {
			settingsError = messageOf(failure);
		}
	}

	function askToRename(event: SubmitEvent) {
		event.preventDefault();
		confirmation = {
			name: 'rename',
			title: t.renameTitle(slug),
			detail: t.renameDetail,
			action: t.rename,
			variant: 'secondary',
			run: rename
		};
	}
</script>

<svelte:head>
	<title>{instance.slug} - MCP Hub</title>
</svelte:head>

<div class="cb-crumbs">
	<a href={resolve('/(app)/instances')}>{i18n.t.instances.crumb}</a> /
	<span class="crumb-here">{instance.slug}</span>
</div>

<div class="cb-page-head">
	<div>
		<span class="cb-label">{t.label}</span>
		<h1 class="cb-page-title">{instance.slug}</h1>
		<div class="cb-meta">
			<span>{t.metaCatalog(instance.server_id)}</span>
			{#if status.mode}
				<span class="cb-meta-sep">|</span>
				<span>{t.metaMode(status.mode)}</span>
			{/if}
			{#if status.runtime}
				<span class="cb-meta-sep">|</span>
				<span>{t.metaRuntime(status.runtime)}</span>
			{/if}
		</div>
	</div>
	<div class="cb-actions">
		<StatusBadge status={label} />
		{#if !cannotStart}
			{#if status.enabled}
				<Button busy={pendingAction === 'restart'} onclick={askToRestart}>
					{pendingAction === 'restart' ? t.restarting : t.restart}
				</Button>
				<Button variant="danger" busy={pendingAction === 'stop'} onclick={askToStop}>
					{pendingAction === 'stop' ? t.stopping : t.stop}
				</Button>
			{:else}
				<Button busy={pendingAction === 'start'} onclick={start}>
					{pendingAction === 'start' ? t.starting : t.start}
				</Button>
			{/if}
		{/if}
	</div>
</div>

{#if instance.needs_attention === 'server_removed'}
	<Banner tone="warn" tag={t.needsAttention}>
		<strong>{t.serverRemovedLead}</strong>
		{t.serverRemovedBody}
	</Banner>
{:else if instance.needs_attention === 'server_disabled'}
	<Banner tone="warn" tag={t.needsAttention}>
		<strong>{t.serverDisabledLead}</strong>
		{t.serverDisabledBody}
	</Banner>
{:else if instance.needs_attention === 'secrets_reset'}
	<Banner tone="warn" tag={t.needsAttention}>
		<strong>{t.secretsResetLead}</strong>
		{t.secretsResetBody}
	</Banner>
{/if}

{#if instance.missing_required.length > 0 && !cannotStart}
	<Banner tone="warn" tag={t.needsAttention}>
		<strong>{t.missingLead}</strong>
		{t.missingBody(instance.missing_required.join(', '))}
	</Banner>
{/if}

<div class="cb-cols">
	<div class="cb-col-main">
		<section class="cb-panel">
			<div class="cb-panel-head">
				<h2 class="cb-label">{t.connection}</h2>
				<span class="cb-hint">OAuth 2.1</span>
			</div>
			<div class="cb-panel-body">
				<CodeBlock value={instance.url} name={t.connectionUrl} />
			</div>
			<ClientGuides slug={instance.slug} url={instance.url} />
		</section>

		{#if server || instance.env.length > 0}
			<EnvVars
				instanceId={instance.id}
				slug={instance.slug}
				env={instance.env}
				specs={server?.env ?? []}
				missingRequired={instance.missing_required}
				liveProcesses={status.live_processes}
				onchange={invalidateAll}
				onrestart={askToRestart}
			/>
		{/if}

		<ToolFilter instanceId={instance.id} {tools} />

		<form class="cb-panel" onsubmit={askToRename}>
			<div class="cb-panel-head">
				<h2 class="cb-label">{t.settings}</h2>
			</div>
			<div class="fields">
				<SlugField id="settings-slug" bind:value={slug} error={settingsError} />
			</div>
			<div class="cb-panel-foot">
				<Button type="submit" disabled={slug === instance.slug}>{t.saveSettings}</Button>
			</div>
		</form>

		<section class="cb-panel">
			<div class="cb-panel-head">
				<h2 class="cb-label">{t.deleteHeading}</h2>
			</div>
			<div class="cb-panel-body delete">
				<p>{t.deleteBody}</p>
				<Button variant="danger" busy={pendingAction === 'delete'} onclick={askToDelete}>
					{pendingAction === 'delete' ? t.deleting : t.deleteHeading}
				</Button>
			</div>
		</section>
	</div>

	<div class="cb-col-side">
		<section class="cb-panel">
			<div class="cb-panel-head">
				<h2 class="cb-label">{t.statusPanel}</h2>
				<span class="cb-live">{t.live}</span>
			</div>
			<dl class="cb-kv-list">
				<div class="cb-kv">
					<dt>{t.kvStatus}</dt>
					<dd><StatusBadge status={label} /></dd>
				</div>
				<div class="cb-kv">
					<dt>{t.kvMode}</dt>
					<dd>{status.mode ?? '-'}</dd>
				</div>
				<div class="cb-kv">
					<dt>{t.kvRuntime}</dt>
					<dd>{status.runtime ?? '-'}</dd>
				</div>
				<div class="cb-kv">
					<dt>{t.kvPackage}</dt>
					<dd>{instance.package ?? '-'}</dd>
				</div>
				<div class="cb-kv">
					<dt>{t.kvProcesses}</dt>
					<dd>{t.liveCount(status.live_processes)}</dd>
				</div>
				<div class={['cb-kv', lastFailure && 'cb-kv--err']}>
					<dt>{t.kvLastExit}</dt>
					<dd>{status.last_exit ?? '-'}</dd>
				</div>
			</dl>
		</section>

		<LogPanel output={status.output_tail} />
	</div>
</div>

{#if confirmation}
	{@const current = confirmation}
	<ConfirmDialog
		context={i18n.t.common.instanceContext(instance.slug)}
		title={current.title}
		detail={current.detail}
		action={current.action}
		variant={current.variant}
		onconfirm={() => act(current.name, current.run)}
		oncancel={() => (confirmation = null)}
	/>
{/if}

<style>
	.crumb-here {
		color: var(--text);
	}
	.fields {
		display: flex;
		flex-direction: column;
		gap: var(--space-5);
		padding: var(--space-5);
	}
	.delete {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-4);
		font-size: 12px;
		line-height: 18px;
		color: var(--text-2);
	}
</style>

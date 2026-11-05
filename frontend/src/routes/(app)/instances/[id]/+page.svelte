<script lang="ts">
	import { api, messageOf } from '#lib/api/client.ts';
	import { RUNTIMES, type InstanceStatus, type Runtime } from '#lib/api/types.ts';
	import Banner from '#lib/components/Banner.svelte';
	import Button from '#lib/components/Button.svelte';
	import ClientGuides from '#lib/components/ClientGuides.svelte';
	import CodeBlock from '#lib/components/CodeBlock.svelte';
	import ConfirmDialog from '#lib/components/ConfirmDialog.svelte';
	import EnvVars from '#lib/components/EnvVars.svelte';
	import LogPanel from '#lib/components/LogPanel.svelte';
	import SegmentedControl from '#lib/components/SegmentedControl.svelte';
	import SlugField from '#lib/components/SlugField.svelte';
	import StatusBadge from '#lib/components/StatusBadge.svelte';
	import { failedExit, statusLabel } from '#lib/status.ts';
	import { toasts } from '#lib/toasts.svelte.ts';
	import { goto, invalidateAll } from '$app/navigation';
	import { resolve } from '$app/paths';
	import type { PageProps } from './$types';

	const STATUS_POLL_MS = 5000;

	type Confirmation = {
		title: string;
		detail: string;
		action: string;
		variant?: 'danger' | 'secondary';
		run: () => Promise<void>;
	};

	let { data }: PageProps = $props();

	const instance = $derived(data.instance);
	const server = $derived(data.servers.find((candidate) => candidate.id === instance.server_id));
	const path = $derived(`/instances/${instance.id}`);

	let status = $derived<InstanceStatus>(data.status);
	const label = $derived(statusLabel(status));
	const lastFailure = $derived(failedExit(status));
	const isServerRemoved = $derived(instance.needs_attention === 'server_removed');

	// Unsaved edits follow data.status, not the polled status, so polling can't overwrite them.
	const savedRuntime = $derived(data.status.runtime);
	let slug = $derived(instance.slug);
	let runtime = $derived<Runtime>(savedRuntime ?? 'bun');
	const hasSettingChanges = $derived(slug !== instance.slug || runtime !== savedRuntime);
	let settingsError = $state('');

	let confirmation = $state.raw<Confirmation | null>(null);
	let pendingAction = $state<string | null>(null);

	// Polls while this page is open and visible. Background tabs stay quiet.
	$effect(() => {
		const id = instance.id;
		const timer = setInterval(async () => {
			if (document.visibilityState !== 'visible') {
				return;
			}
			try {
				status = await api.get<InstanceStatus>(`/instances/${id}/status`);
			} catch {
				// The next tick tries again; a lost session is already redirected by the client.
			}
		}, STATUS_POLL_MS);
		return () => clearInterval(timer);
	});

	async function act(name: string, action: () => Promise<void>) {
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

	const liveNote = $derived.by(() => {
		const count = status.live_processes;
		if (count === 0) {
			return '';
		}
		return count === 1 ? '1 live process will end. ' : `${count} live processes will end. `;
	});

	const start = () => act('start', () => api.post(`${path}/start`));

	function askToStop() {
		confirmation = {
			title: 'Stop instance?',
			detail: `${liveNote}Connected apps lose access until you start it again.`,
			action: 'Stop instance',
			run: () => api.post(`${path}/stop`)
		};
	}

	function askToRestart() {
		confirmation = {
			title: 'Restart instance?',
			detail: `${liveNote}Connected apps reconnect by themselves, and work in progress is cancelled.`,
			action: 'Restart instance',
			variant: 'secondary',
			run: async () => {
				await api.post(`${path}/restart`);
				toasts.show('ok', `${instance.slug} restarted`);
			}
		};
	}

	function askToDelete() {
		confirmation = {
			title: 'Delete instance?',
			detail: 'Its saved values are deleted and every app using it is disconnected.',
			action: 'Delete instance',
			run: async () => {
				await api.delete(path);
				toasts.show('ok', `${instance.slug} deleted`);
				await goto(resolve('/(app)/instances'));
			}
		};
	}

	async function saveSettings() {
		settingsError = '';
		const changes = {
			slug: slug === instance.slug ? undefined : slug,
			runtime: runtime === savedRuntime ? undefined : runtime
		};
		try {
			await api.patch(path, changes);
			toasts.show('ok', 'Settings saved');
		} catch (failure) {
			settingsError = messageOf(failure);
		}
	}

	function askToSaveSettings(event: SubmitEvent) {
		event.preventDefault();
		const isRename = slug !== instance.slug;
		confirmation = {
			title: isRename ? `Rename to ${slug}?` : `Switch to ${runtime}?`,
			detail: isRename
				? `${liveNote}The connection URL changes, so every connected app has to be added again.`
				: `${liveNote}Connected apps reconnect by themselves.`,
			action: 'Save settings',
			variant: 'secondary',
			run: saveSettings
		};
	}
</script>

<svelte:head>
	<title>{instance.slug} - MCP Hub</title>
</svelte:head>

<div class="cb-crumbs">
	<a href={resolve('/(app)/instances')}>instances</a> /
	<span class="crumb-here">{instance.slug}</span>
</div>

<div class="cb-page-head">
	<div>
		<span class="cb-label">Instance</span>
		<h1 class="cb-page-title">{instance.slug}</h1>
		<div class="cb-meta">
			<span>catalog: {instance.server_id}</span>
			{#if status.mode}
				<span class="cb-meta-sep">|</span>
				<span>mode: {status.mode}</span>
			{/if}
			{#if status.runtime}
				<span class="cb-meta-sep">|</span>
				<span>runtime: {status.runtime}</span>
			{/if}
		</div>
	</div>
	<div class="cb-actions">
		<StatusBadge status={label} />
		{#if !isServerRemoved}
			{#if status.enabled}
				<Button busy={pendingAction === 'Restart instance'} onclick={askToRestart}>
					{pendingAction === 'Restart instance' ? 'Restarting' : 'Restart'}
				</Button>
				<Button variant="danger" busy={pendingAction === 'Stop instance'} onclick={askToStop}>
					{pendingAction === 'Stop instance' ? 'Stopping' : 'Stop'}
				</Button>
			{:else}
				<Button busy={pendingAction === 'start'} onclick={start}>
					{pendingAction === 'start' ? 'Starting' : 'Start'}
				</Button>
			{/if}
		{/if}
	</div>
</div>

{#if isServerRemoved}
	<Banner tone="warn" tag="Needs attention">
		<strong>This server was removed from the catalog.</strong> It can't start until the operator adds
		it back. You can still delete this instance.
	</Banner>
{/if}

<div class="cb-cols">
	<div class="cb-col-main">
		<section class="cb-panel">
			<div class="cb-panel-head">
				<h2 class="cb-label">Connection</h2>
				<span class="cb-hint">OAuth 2.1</span>
			</div>
			<div class="cb-panel-body">
				<CodeBlock value={instance.url} name="connection URL" />
			</div>
			<ClientGuides slug={instance.slug} url={instance.url} />
		</section>

		<EnvVars
			instanceId={instance.id}
			slug={instance.slug}
			env={instance.env}
			allowedVars={server?.env_vars ?? []}
			liveProcesses={status.live_processes}
			onchange={invalidateAll}
			onrestart={askToRestart}
		/>

		<form class="cb-panel" onsubmit={askToSaveSettings}>
			<div class="cb-panel-head">
				<h2 class="cb-label">Settings</h2>
			</div>
			<div class="fields">
				<SlugField id="settings-slug" bind:value={slug} error={settingsError} />
				<div class="cb-field">
					<span class="cb-label">Runtime</span>
					<div>
						<SegmentedControl
							label="Runtime"
							name="settings-runtime"
							options={RUNTIMES}
							bind:value={runtime}
						/>
					</div>
				</div>
			</div>
			<div class="cb-panel-foot">
				<Button type="submit" disabled={!hasSettingChanges}>Save settings</Button>
			</div>
		</form>

		<section class="cb-panel">
			<div class="cb-panel-head">
				<h2 class="cb-label">Delete instance</h2>
			</div>
			<div class="cb-panel-body delete">
				<p>Removes its saved values and its connection URL. This can't be undone.</p>
				<Button variant="danger" busy={pendingAction === 'Delete instance'} onclick={askToDelete}>
					{pendingAction === 'Delete instance' ? 'Deleting' : 'Delete instance'}
				</Button>
			</div>
		</section>
	</div>

	<div class="cb-col-side">
		<section class="cb-panel">
			<div class="cb-panel-head">
				<h2 class="cb-label">Status</h2>
				<span class="cb-live">live</span>
			</div>
			<dl class="cb-kv-list">
				<div class="cb-kv">
					<dt>Status</dt>
					<dd><StatusBadge status={label} /></dd>
				</div>
				<div class="cb-kv">
					<dt>Mode</dt>
					<dd>{status.mode ?? '-'}</dd>
				</div>
				<div class="cb-kv">
					<dt>Runtime</dt>
					<dd>{status.runtime ?? '-'}</dd>
				</div>
				<div class="cb-kv">
					<dt>Processes</dt>
					<dd>{status.live_processes} live</dd>
				</div>
				<div class={['cb-kv', lastFailure && 'cb-kv--err']}>
					<dt>Last exit</dt>
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
		context={`Instance ${instance.slug}`}
		title={current.title}
		detail={current.detail}
		action={current.action}
		variant={current.variant}
		onconfirm={() => act(current.action, current.run)}
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

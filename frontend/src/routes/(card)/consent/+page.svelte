<script lang="ts">
	import { api, messageOf } from '#lib/api/client.ts';
	import type { ConsentDecision } from '#lib/api/types.ts';
	import Banner from '#lib/components/Banner.svelte';
	import Button from '#lib/components/Button.svelte';
	import { i18n } from '#lib/i18n/index.svelte.ts';
	import { invalidateAll } from '$app/navigation';
	import { resolve } from '$app/paths';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	const pending = $derived(data.pending);
	const clientName = $derived(pending?.client_name_self_reported ?? i18n.t.consent.unnamed);

	let error = $state('');
	let deciding = $state<'approve' | 'deny' | null>(null);

	async function decide(decision: 'approve' | 'deny') {
		deciding = decision;
		error = '';
		try {
			const { redirect_url } = await api.post<ConsentDecision>(
				`/oauth/pending/${encodeURIComponent(data.requestId)}/${decision}`
			);
			// This leaves the site on purpose, for the app that asked.
			window.location.assign(redirect_url);
		} catch (failure) {
			error = messageOf(failure);
			deciding = null;
		}
	}
</script>

<svelte:head>
	<title>{i18n.t.consent.pageTitle}</title>
</svelte:head>

{#snippet failure()}
	{#if error}
		<Banner tone="danger" tag={i18n.t.consent.failed}>{error}</Banner>
	{/if}
{/snippet}

{#snippet clientRow()}
	<div class="cb-kv">
		<dt>{i18n.t.consent.client}</dt>
		<dd>{clientName} <span class="cb-tag">{i18n.t.common.selfReported}</span></dd>
	</div>
{/snippet}

<div class="cb-card">
	{#if !pending}
		<div class="cb-card-head">
			<span class="cb-label">{i18n.t.consent.label}</span>
			<h1 class="cb-card-title">{i18n.t.consent.expiredTitle}</h1>
		</div>
		<div class="cb-card-body">
			<p class="note">{i18n.t.consent.expiredBody}</p>
		</div>
	{:else if !pending.instance}
		<div class="cb-card-head">
			<span class="cb-label">{i18n.t.consent.label}</span>
			<h1 class="cb-card-title">{i18n.t.consent.noInstanceTitle(pending.requested_slug ?? '')}</h1>
		</div>
		<div class="cb-card-body">
			<dl class="cb-kv-list boxed">{@render clientRow()}</dl>
			<p class="note">{i18n.t.consent.noInstanceBody}</p>
			{@render failure()}
			<a
				class="cb-btn cb-btn--secondary cb-btn--lg cb-btn--block"
				href="{resolve('/(app)/instances/new')}?slug={encodeURIComponent(
					pending.requested_slug ?? ''
				)}"
				target="_blank"
				rel="noopener"
			>
				{i18n.t.consent.createInstance}
			</a>
			<div class="cb-consent-actions">
				<Button busy={deciding === 'deny'} onclick={() => decide('deny')}>
					{deciding === 'deny' ? i18n.t.consent.denying : i18n.t.consent.deny}
				</Button>
				<Button onclick={() => invalidateAll()}>{i18n.t.consent.checkAgain}</Button>
			</div>
		</div>
	{:else}
		<div class="cb-card-head">
			<span class="cb-label">{i18n.t.consent.label}</span>
			<h1 class="cb-card-title">
				{i18n.t.consent.title(clientName, pending.instance.slug)}
			</h1>
		</div>
		<div class="cb-card-body">
			<dl class="cb-kv-list boxed">
				{@render clientRow()}
				<div class="cb-kv">
					<dt>{i18n.t.consent.instance}</dt>
					<dd>
						<strong>{pending.instance.slug}</strong>
						<span class="cb-muted">{pending.instance.server_id}</span>
					</dd>
				</div>
			</dl>

			{#if pending.redirect_is_loopback}
				<div class="cb-redirect cb-redirect--warn">
					<span class="cb-label warn-label">{i18n.t.consent.loopbackLabel}</span>
					<span class="cb-redirect-host">{pending.redirect_host}</span>
					<span class="cb-field-help warn-help">
						{i18n.t.consent.loopbackHelp}
					</span>
				</div>
			{:else}
				<div class="cb-redirect">
					<span class="cb-label">{i18n.t.consent.redirectLabel}</span>
					<span class="cb-redirect-host">{pending.redirect_host}</span>
					<span class="cb-field-help">
						{i18n.t.consent.redirectHelp}
					</span>
				</div>
			{/if}

			<p class="note">
				{i18n.t.consent.scope}
			</p>

			{@render failure()}

			<!-- Equal weight on purpose: a hurried tap must not be steered toward Approve. -->
			<div class="cb-consent-actions">
				<Button
					busy={deciding === 'deny'}
					disabled={deciding === 'approve'}
					onclick={() => decide('deny')}
				>
					{deciding === 'deny' ? i18n.t.consent.denying : i18n.t.consent.deny}
				</Button>
				<Button
					busy={deciding === 'approve'}
					disabled={deciding === 'deny'}
					onclick={() => decide('approve')}
				>
					{deciding === 'approve' ? i18n.t.consent.approving : i18n.t.consent.approve}
				</Button>
			</div>
		</div>
	{/if}
	<div class="cb-card-foot">{i18n.t.consent.signedInAs(data.user?.username ?? '')}</div>
</div>

<style>
	.boxed {
		border: var(--bw) solid var(--border);
	}
	.boxed .cb-kv {
		padding: var(--space-1) var(--space-3);
	}
	.note {
		font-size: 12px;
		line-height: 18px;
		color: var(--text-2);
	}
	.warn-label {
		color: var(--warn);
	}
	.warn-help {
		color: var(--text);
	}
</style>

<script lang="ts">
	import { api, messageOf } from '#lib/api/client.ts';
	import type { ConsentDecision } from '#lib/api/types.ts';
	import Banner from '#lib/components/Banner.svelte';
	import Button from '#lib/components/Button.svelte';
	import { invalidateAll } from '$app/navigation';
	import { resolve } from '$app/paths';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	const pending = $derived(data.pending);
	const clientName = $derived(pending?.client_name_self_reported ?? 'An unnamed app');

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
	<title>Authorize access - MCP Hub</title>
</svelte:head>

{#snippet failure()}
	{#if error}
		<Banner tone="danger" tag="Failed">{error}</Banner>
	{/if}
{/snippet}

{#snippet clientRow()}
	<div class="cb-kv">
		<dt>Client</dt>
		<dd>{clientName} <span class="cb-tag">Self-reported</span></dd>
	</div>
{/snippet}

<div class="cb-card">
	{#if !pending}
		<div class="cb-card-head">
			<span class="cb-label">Authorize access</span>
			<h1 class="cb-card-title">This request has expired</h1>
		</div>
		<div class="cb-card-body">
			<p class="note">Start connecting again from your app.</p>
		</div>
	{:else if !pending.instance}
		<div class="cb-card-head">
			<span class="cb-label">Authorize access</span>
			<h1 class="cb-card-title">You have no instance called {pending.requested_slug}</h1>
		</div>
		<div class="cb-card-body">
			<dl class="cb-kv-list boxed">{@render clientRow()}</dl>
			<p class="note">It asked for this instance. Create it, then check again.</p>
			{@render failure()}
			<a
				class="cb-btn cb-btn--secondary cb-btn--lg cb-btn--block"
				href="{resolve('/(app)/instances/new')}?slug={encodeURIComponent(
					pending.requested_slug ?? ''
				)}"
				target="_blank"
				rel="noopener"
			>
				Create instance
			</a>
			<div class="cb-consent-actions">
				<Button busy={deciding === 'deny'} onclick={() => decide('deny')}>
					{deciding === 'deny' ? 'Denying' : 'Deny'}
				</Button>
				<Button onclick={() => invalidateAll()}>Check again</Button>
			</div>
		</div>
	{:else}
		<div class="cb-card-head">
			<span class="cb-label">Authorize access</span>
			<h1 class="cb-card-title">
				Allow {clientName} to use your {pending.instance.slug} instance?
			</h1>
		</div>
		<div class="cb-card-body">
			<dl class="cb-kv-list boxed">
				{@render clientRow()}
				<div class="cb-kv">
					<dt>Instance</dt>
					<dd>
						<strong>{pending.instance.slug}</strong>
						<span class="cb-muted">{pending.instance.server_id}</span>
					</dd>
				</div>
			</dl>

			{#if pending.redirect_is_loopback}
				<div class="cb-redirect cb-redirect--warn">
					<span class="cb-label warn-label">[ ! ] Redirects to your computer</span>
					<span class="cb-redirect-host">{pending.redirect_host}</span>
					<span class="cb-field-help warn-help">
						An app on your own computer will receive access. Only approve if you just started this
						sign-in from a tool on this device.
					</span>
				</div>
			{:else}
				<div class="cb-redirect">
					<span class="cb-label">You will be sent back to</span>
					<span class="cb-redirect-host">{pending.redirect_host}</span>
					<span class="cb-field-help">
						The client name is self-reported. Trust the redirect host, not the name.
					</span>
				</div>
			{/if}

			<p class="note">
				It can call every tool on this instance as you. Stop the instance to cut off access.
			</p>

			{@render failure()}

			<!-- Equal weight on purpose: a hurried tap must not be steered toward Approve. -->
			<div class="cb-consent-actions">
				<Button
					busy={deciding === 'deny'}
					disabled={deciding === 'approve'}
					onclick={() => decide('deny')}
				>
					{deciding === 'deny' ? 'Denying' : 'Deny'}
				</Button>
				<Button
					busy={deciding === 'approve'}
					disabled={deciding === 'deny'}
					onclick={() => decide('approve')}
				>
					{deciding === 'approve' ? 'Approving' : 'Approve'}
				</Button>
			</div>
		</div>
	{/if}
	<div class="cb-card-foot">Signed in as {data.user?.username}</div>
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

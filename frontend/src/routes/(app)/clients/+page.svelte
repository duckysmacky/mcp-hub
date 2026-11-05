<script lang="ts">
	import { api, messageOf } from '#lib/api/client.ts';
	import type { Grant } from '#lib/api/types.ts';
	import Button from '#lib/components/Button.svelte';
	import ConfirmDialog from '#lib/components/ConfirmDialog.svelte';
	import { relativeTime } from '#lib/dates.ts';
	import { i18n } from '#lib/i18n/index.svelte.ts';
	import { toasts } from '#lib/toasts.svelte.ts';
	import { invalidateAll } from '$app/navigation';
	import { resolve } from '$app/paths';
	import type { PageProps } from './$types';

	const LOOPBACK_HOSTS = ['localhost', '127.0.0.1', '[::1]', '::1'];

	let { data }: PageProps = $props();

	const t = $derived(i18n.t.clients);
	const nameOf = (grant: Grant) => grant.client_name_self_reported ?? t.unnamed;

	/** Only a hostname, as text: the URL is unverified, so it's not offered as a link. */
	function reportedHost(grant: Grant): string | null {
		try {
			return new URL(grant.client_uri_self_reported ?? '').hostname || null;
		} catch {
			return null;
		}
	}

	let revoking = $state.raw<Grant | null>(null);
	let pendingId = $state<number | null>(null);

	async function revoke(grant: Grant) {
		revoking = null;
		pendingId = grant.id;
		try {
			await api.delete(`/grants/${grant.id}`);
			await invalidateAll();
			toasts.show('ok', t.revoked(nameOf(grant)));
		} catch (failure) {
			toasts.show('err', messageOf(failure));
		} finally {
			pendingId = null;
		}
	}
</script>

<svelte:head>
	<title>{t.pageTitle}</title>
</svelte:head>

<div class="cb-page-head">
	<h1 class="cb-page-title">{t.title}</h1>
</div>

{#if data.grants.length === 0}
	<div class="cb-empty">
		<span class="cb-empty-mark">{t.emptyMark}</span>
		<h2 class="cb-empty-title">{t.emptyTitle}</h2>
		<p class="cb-empty-body">{t.emptyBody}</p>
	</div>
{:else}
	<section class="cb-panel">
		<div class="cb-table-wrap">
			<table class="cb-table">
				<thead>
					<tr>
						<th>{t.client}</th>
						<th>{t.redirectHost}</th>
						<th>{t.instance}</th>
						<th>{t.created}</th>
						<th>{t.lastUsed}</th>
						<th><span class="cb-hint">{t.actions}</span></th>
					</tr>
				</thead>
				<tbody>
					{#each data.grants as grant (grant.id)}
						{@const host = reportedHost(grant)}
						<tr>
							<td data-label={t.client}>
								<div>
									<strong>{nameOf(grant)}</strong>
									<span class="cb-tag">{i18n.t.common.selfReported}</span>
								</div>
								{#if host}<div class="cb-hint">{host}</div>{/if}
							</td>
							<td data-label={t.redirectHost}>
								{#each grant.redirect_hosts as redirectHost (redirectHost)}
									<div>
										{redirectHost}
										{#if LOOPBACK_HOSTS.includes(redirectHost)}
											<span class="cb-tag cb-tag--warn">{i18n.t.common.localhost}</span>
										{/if}
									</div>
								{/each}
							</td>
							<td data-label={t.instance}>
								<a href={resolve('/(app)/instances/[id]', { id: String(grant.instance_id) })}>
									{grant.slug}
								</a>
								<div class="cb-hint">{grant.server_id}</div>
							</td>
							<td data-label={t.created}>{grant.created_at.slice(0, 10)}</td>
							<td data-label={t.lastUsed}>
								{grant.last_used_at ? relativeTime(grant.last_used_at, i18n.locale) : t.never}
							</td>
							<td class="cb-num">
								<Button
									variant="ghost-danger"
									size="sm"
									busy={pendingId === grant.id}
									onclick={() => (revoking = grant)}
								>
									{pendingId === grant.id ? t.revoking : t.revoke}
								</Button>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	</section>
{/if}

{#if revoking}
	{@const grant = revoking}
	<ConfirmDialog
		context={t.title}
		title={t.revokeTitle(nameOf(grant))}
		detail={t.revokeDetail(grant.slug)}
		action={t.revoke}
		onconfirm={() => revoke(grant)}
		oncancel={() => (revoking = null)}
	/>
{/if}

<style>
	td {
		padding-block: var(--space-2);
	}
	/* Cobalt: on phones, a table of this width stacks its rows as cards. */
	@media (max-width: 639px) {
		thead {
			display: none;
		}
		tr,
		td {
			display: block;
		}
		td {
			height: auto;
			padding: var(--space-1) var(--space-4);
			border-bottom: 0;
			white-space: normal;
			overflow-wrap: anywhere;
			text-align: left;
		}
		td[data-label]::before {
			content: attr(data-label);
			display: block;
			font-size: 10px;
			line-height: 14px;
			font-weight: 700;
			letter-spacing: 0.08em;
			text-transform: uppercase;
			color: var(--text-3);
		}
		tr {
			padding-block: var(--space-3);
		}
		tr + tr {
			border-top: var(--bw) solid var(--border);
		}
	}
</style>

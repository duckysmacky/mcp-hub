<script lang="ts">
	import { api } from '#lib/api/client.ts';
	import Brand from '#lib/components/Brand.svelte';
	import Button from '#lib/components/Button.svelte';
	import Toasts from '#lib/components/Toasts.svelte';
	import { i18n } from '#lib/i18n/index.svelte.ts';
	import { goto, invalidateAll } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import type { LayoutProps } from './$types';

	let { data, children }: LayoutProps = $props();

	const inSection = (section: string) => page.url.pathname.startsWith(section);

	async function signOut() {
		try {
			await api.post('/auth/logout');
		} finally {
			await invalidateAll();
			await goto(resolve('/(card)/login'));
		}
	}
</script>

{#snippet sectionLink(className: string)}
	<a
		class={className}
		href={resolve('/(app)/instances')}
		aria-current={inSection('/instances') ? 'page' : undefined}
	>
		{i18n.t.nav.instances}
	</a>
	<a
		class={className}
		href={resolve('/(app)/clients')}
		aria-current={inSection('/clients') ? 'page' : undefined}
	>
		{i18n.t.nav.clients}
	</a>
	<a
		class={className}
		href={resolve('/(app)/settings')}
		aria-current={inSection('/settings') ? 'page' : undefined}
	>
		{i18n.t.nav.settings}
	</a>
{/snippet}

<header class="cb-nav">
	<div class="cb-nav-in">
		<Brand />
		<nav class="cb-nav-links" aria-label={i18n.t.nav.primary}>
			{@render sectionLink('cb-nav-link')}
		</nav>
		<div class="cb-nav-user">
			<span>{data.user?.username}</span>
			<Button variant="ghost" size="sm" onclick={signOut}>{i18n.t.nav.signOut}</Button>
		</div>
		<button class="cb-btn cb-btn--ghost cb-menu-btn" type="button" popovertarget="account-menu">
			{data.user?.username}
		</button>
	</div>
</header>

<div class="cb-panel account-menu" id="account-menu" popover="auto">
	<Button variant="ghost" block onclick={signOut}>{i18n.t.nav.signOut}</Button>
</div>

<main class="cb-page">
	{@render children()}
</main>

<nav class="cb-tabbar" aria-label={i18n.t.nav.primary}>
	{@render sectionLink('cb-tab')}
</nav>

<Toasts />

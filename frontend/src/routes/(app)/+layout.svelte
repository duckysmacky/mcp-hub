<script lang="ts">
	import { api } from '#lib/api/client.ts';
	import Brand from '#lib/components/Brand.svelte';
	import Button from '#lib/components/Button.svelte';
	import Toasts from '#lib/components/Toasts.svelte';
	import { goto, invalidateAll } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import type { LayoutProps } from './$types';

	let { data, children }: LayoutProps = $props();

	const inInstances = $derived(page.url.pathname.startsWith('/instances'));

	// app.html sets this before first paint.
	let theme = $state(document.documentElement.dataset.theme);
	const otherThemeLabel = $derived(theme === 'light' ? 'Dark theme' : 'Light theme');

	function toggleTheme() {
		theme = theme === 'light' ? 'dark' : 'light';
		document.documentElement.dataset.theme = theme;
		try {
			localStorage.setItem('theme', theme);
		} catch {
			// Storage can be blocked: the choice just lasts until the next reload.
		}
	}

	async function signOut() {
		try {
			await api.post('/auth/logout');
		} finally {
			await invalidateAll();
			await goto(resolve('/(card)/login'));
		}
	}
</script>

<header class="cb-nav">
	<div class="cb-nav-in">
		<Brand />
		<nav class="cb-nav-links" aria-label="Primary">
			<a
				class="cb-nav-link"
				href={resolve('/(app)/instances')}
				aria-current={inInstances ? 'page' : undefined}
			>
				Instances
			</a>
		</nav>
		<div class="cb-nav-user">
			<span>{data.user?.username}</span>
			<Button variant="ghost" size="sm" onclick={toggleTheme}>{otherThemeLabel}</Button>
			<Button variant="ghost" size="sm" onclick={signOut}>Sign out</Button>
		</div>
		<button class="cb-btn cb-btn--ghost cb-menu-btn" type="button" popovertarget="account-menu">
			{data.user?.username}
		</button>
	</div>
</header>

<div class="cb-panel account-menu" id="account-menu" popover="auto">
	<Button variant="ghost" block onclick={toggleTheme}>{otherThemeLabel}</Button>
	<Button variant="ghost" block onclick={signOut}>Sign out</Button>
</div>

<main class="cb-page">
	{@render children()}
</main>

<Toasts />

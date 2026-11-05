<script lang="ts">
	import Brand from '#lib/components/Brand.svelte';
	import { i18n } from '#lib/i18n/index.svelte.ts';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';

	const message = $derived(
		page.status === 404
			? i18n.t.errors.pageMissing
			: (page.error?.message ?? i18n.t.errors.pageFailed)
	);
</script>

<svelte:head>
	<title>{message} - MCP Hub</title>
</svelte:head>

<main class="cb-auth">
	<div class="cb-card">
		<div class="cb-card-head">
			<Brand />
			<span class="cb-label">{i18n.t.errors.errorLabel(page.status)}</span>
			<h1 class="cb-card-title">{message}</h1>
		</div>
		<div class="cb-card-body">
			<a
				class="cb-btn cb-btn--secondary cb-btn--lg cb-btn--block"
				href={resolve('/(app)/instances')}
			>
				{i18n.t.errors.goToInstances}
			</a>
		</div>
	</div>
</main>

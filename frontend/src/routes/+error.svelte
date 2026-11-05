<script lang="ts">
	import Brand from '#lib/components/Brand.svelte';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';

	const message = $derived(
		page.status === 404 ? 'This page doesn’t exist' : (page.error?.message ?? 'The request failed')
	);
</script>

<svelte:head>
	<title>{message} - MCP Hub</title>
</svelte:head>

<main class="cb-auth">
	<div class="cb-card">
		<div class="cb-card-head">
			<Brand />
			<span class="cb-label">Error {page.status}</span>
			<h1 class="cb-card-title">{message}</h1>
		</div>
		<div class="cb-card-body">
			<a class="cb-btn cb-btn--secondary cb-btn--lg cb-btn--block" href={resolve('/instances')}>
				Go to instances
			</a>
		</div>
	</div>
</main>

<script lang="ts">
	import AuthForm from '#lib/components/AuthForm.svelte';
	import Banner from '#lib/components/Banner.svelte';
	import Brand from '#lib/components/Brand.svelte';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
</script>

<svelte:head>
	<title>Create account - MCP Hub</title>
</svelte:head>

{#if data.config.signup_enabled}
	<AuthForm mode="signup">
		{#snippet foot()}
			Have an account? <a href="{resolve('/(card)/login')}{page.url.search}">Sign in</a>
		{/snippet}
	</AuthForm>
{:else}
	<div class="cb-card">
		<div class="cb-card-head">
			<Brand />
			<h1 class="cb-card-title">Create account</h1>
		</div>
		<div class="cb-card-body">
			<Banner tone="info" tag="Sign-up closed">
				The operator of this hub has turned off new accounts. Ask them to set one up for you.
			</Banner>
			<a
				class="cb-btn cb-btn--secondary cb-btn--lg cb-btn--block"
				href="{resolve('/(card)/login')}{page.url.search}"
			>
				Back to sign in
			</a>
		</div>
	</div>
{/if}

<script lang="ts">
	import AuthForm from '#lib/components/AuthForm.svelte';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
</script>

<svelte:head>
	<title>Sign in - MCP Hub</title>
</svelte:head>

{#if data.config.signup_enabled}
	<AuthForm mode="login">
		{#snippet foot()}
			No account? <a href="{resolve('/(card)/signup')}{page.url.search}">Create one</a>
		{/snippet}
	</AuthForm>
{:else}
	<AuthForm mode="login" />
{/if}

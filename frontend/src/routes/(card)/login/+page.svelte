<script lang="ts">
	import AuthForm from '#lib/components/AuthForm.svelte';
	import { i18n } from '#lib/i18n/index.svelte.ts';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
</script>

<svelte:head>
	<title>{i18n.t.auth.signInTitle}</title>
</svelte:head>

{#if data.config.signup === 'disabled'}
	<AuthForm mode="login" />
{:else}
	<AuthForm mode="login">
		{#snippet foot()}
			{data.config.signup === 'invite-only' ? i18n.t.auth.haveInvite : i18n.t.auth.noAccount}
			<a href="{resolve('/(card)/signup')}{page.url.search}">{i18n.t.auth.createOne}</a>
		{/snippet}
	</AuthForm>
{/if}

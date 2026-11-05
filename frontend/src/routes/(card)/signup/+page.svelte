<script lang="ts">
	import AuthForm from '#lib/components/AuthForm.svelte';
	import Banner from '#lib/components/Banner.svelte';
	import Brand from '#lib/components/Brand.svelte';
	import { i18n } from '#lib/i18n/index.svelte.ts';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
</script>

<svelte:head>
	<title>{i18n.t.auth.signupTitle}</title>
</svelte:head>

{#if data.config.signup === 'disabled'}
	<div class="cb-card">
		<div class="cb-card-head">
			<Brand />
			<h1 class="cb-card-title">{i18n.t.auth.createAccount}</h1>
		</div>
		<div class="cb-card-body">
			<Banner tone="info" tag={i18n.t.auth.signupClosedTag}>{i18n.t.auth.signupClosed}</Banner>
			<a
				class="cb-btn cb-btn--secondary cb-btn--lg cb-btn--block"
				href="{resolve('/(card)/login')}{page.url.search}"
			>
				{i18n.t.auth.backToSignIn}
			</a>
		</div>
	</div>
{:else}
	<AuthForm mode="signup" needsInvite={data.config.signup === 'invite-only'}>
		{#snippet foot()}
			{i18n.t.auth.haveAccount}
			<a href="{resolve('/(card)/login')}{page.url.search}">{i18n.t.auth.signIn}</a>
		{/snippet}
	</AuthForm>
{/if}

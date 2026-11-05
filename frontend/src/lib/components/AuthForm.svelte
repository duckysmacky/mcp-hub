<script lang="ts">
	import { ApiError, api, messageOf } from '#lib/api/client.ts';
	import Brand from '#lib/components/Brand.svelte';
	import Button from '#lib/components/Button.svelte';
	import SecretInput from '#lib/components/SecretInput.svelte';
	import { i18n } from '#lib/i18n/index.svelte.ts';
	import { safeNextPath } from '#lib/navigation.ts';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import type { Snippet } from 'svelte';

	let {
		mode,
		needsInvite = false,
		foot
	}: { mode: 'login' | 'signup'; needsInvite?: boolean; foot?: Snippet } = $props();

	let username = $state('');
	let password = $state('');
	let inviteCode = $state('');
	let error = $state('');
	let busy = $state(false);

	const isSignup = $derived(mode === 'signup');
	const title = $derived(isSignup ? i18n.t.auth.createAccount : i18n.t.auth.signIn);
	const busyLabel = $derived(isSignup ? i18n.t.auth.creatingAccount : i18n.t.auth.signingIn);

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		busy = true;
		error = '';
		try {
			if (isSignup) {
				await api.post('/auth/signup', {
					username,
					password,
					invite_code: needsInvite ? inviteCode : undefined
				});
			}
			await api.post('/auth/login', { username, password });
			password = '';
			// An arbitrary path on this site, already checked by safeNextPath.
			// eslint-disable-next-line svelte/no-navigation-without-resolve
			await goto(safeNextPath(page.url.searchParams.get('next')), { invalidateAll: true });
		} catch (failure) {
			const wrongCredentials = failure instanceof ApiError && failure.status === 401;
			error = wrongCredentials ? i18n.t.errors.wrongCredentials : messageOf(failure);
		} finally {
			busy = false;
		}
	}
</script>

<form class="cb-card" onsubmit={submit}>
	<div class="cb-card-head">
		<Brand />
		<h1 class="cb-card-title">{title}</h1>
	</div>
	<div class="cb-card-body">
		{#if needsInvite}
			<div class="cb-field">
				<label class="cb-label" for="invite-code">{i18n.t.auth.inviteCode}</label>
				<input
					class="cb-input"
					id="invite-code"
					bind:value={inviteCode}
					autocomplete="off"
					autocapitalize="none"
					spellcheck="false"
					required
					aria-describedby="invite-help"
				/>
				<span class="cb-field-help" id="invite-help">{i18n.t.auth.inviteHelp}</span>
			</div>
		{/if}
		<div class="cb-field">
			<label class="cb-label" for="username">{i18n.t.auth.username}</label>
			<input
				class="cb-input"
				id="username"
				bind:value={username}
				autocomplete="username"
				autocapitalize="none"
				spellcheck="false"
				required
			/>
		</div>
		<div class="cb-field">
			<label class="cb-label" for="password">{i18n.t.auth.password}</label>
			<SecretInput
				id="password"
				bind:value={password}
				autocomplete={isSignup ? 'new-password' : 'current-password'}
				invalid={error !== ''}
				describedBy="password-note"
			/>
			{#if error}
				<span class="cb-field-error" id="password-note" role="alert">{error}</span>
			{:else if isSignup}
				<span class="cb-field-help" id="password-note">{i18n.t.auth.passwordHelp}</span>
			{/if}
		</div>
		<Button type="submit" variant="primary" size="lg" block {busy}>
			{busy ? busyLabel : title}
		</Button>
	</div>
	{#if foot}
		<div class="cb-card-foot">{@render foot()}</div>
	{/if}
</form>

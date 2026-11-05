<script lang="ts">
	import { ApiError, api, messageOf } from '#lib/api/client.ts';
	import Brand from '#lib/components/Brand.svelte';
	import Button from '#lib/components/Button.svelte';
	import SecretInput from '#lib/components/SecretInput.svelte';
	import { safeNextPath } from '#lib/navigation.ts';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import type { Snippet } from 'svelte';

	let { mode, foot }: { mode: 'login' | 'signup'; foot?: Snippet } = $props();

	let username = $state('');
	let password = $state('');
	let error = $state('');
	let busy = $state(false);

	const isSignup = $derived(mode === 'signup');
	const title = $derived(isSignup ? 'Create account' : 'Sign in');
	const busyLabel = $derived(isSignup ? 'Creating account' : 'Signing in');

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		busy = true;
		error = '';
		try {
			if (isSignup) {
				await api.post('/auth/signup', { username, password });
			}
			await api.post('/auth/login', { username, password });
			password = '';
			// An arbitrary path on this site, already checked by safeNextPath.
			// eslint-disable-next-line svelte/no-navigation-without-resolve
			await goto(safeNextPath(page.url.searchParams.get('next')), { invalidateAll: true });
		} catch (failure) {
			const wrongCredentials = failure instanceof ApiError && failure.status === 401;
			error = wrongCredentials ? 'Wrong username or password.' : messageOf(failure);
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
		<div class="cb-field">
			<label class="cb-label" for="username">Username</label>
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
			<label class="cb-label" for="password">Password</label>
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
				<span class="cb-field-help" id="password-note">At least 8 characters.</span>
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

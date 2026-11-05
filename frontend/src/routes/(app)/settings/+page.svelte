<script lang="ts">
	import { api, messageOf } from '#lib/api/client.ts';
	import Button from '#lib/components/Button.svelte';
	import SecretInput from '#lib/components/SecretInput.svelte';
	import SegmentedControl from '#lib/components/SegmentedControl.svelte';
	import { LOCALES, i18n } from '#lib/i18n/index.svelte.ts';
	import { applyTheme, storedTheme, type ThemeChoice } from '#lib/theme.ts';
	import { toasts } from '#lib/toasts.svelte.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	const t = $derived(i18n.t.settings);

	let theme = $state<ThemeChoice>(storedTheme());
	const themeOptions = $derived([
		{ value: 'system' as const, label: t.themeSystem },
		{ value: 'dark' as const, label: t.themeDark },
		{ value: 'light' as const, label: t.themeLight }
	]);

	let currentPassword = $state('');
	let newPassword = $state('');
	let error = $state('');
	// The backend tells them apart only in words, and only a wrong current password says "current".
	let errorOnCurrent = $state(false);
	let busy = $state(false);
	// Remounts the inputs after a save, so a value revealed with Show doesn't leave them unmasked.
	let saveCount = $state(0);

	async function changePassword(event: SubmitEvent) {
		event.preventDefault();
		busy = true;
		error = '';
		try {
			await api.post('/auth/password', {
				current_password: currentPassword,
				new_password: newPassword
			});
			currentPassword = '';
			newPassword = '';
			saveCount += 1;
			toasts.show('ok', t.passwordChanged);
		} catch (failure) {
			error = messageOf(failure);
			errorOnCurrent =
				failure instanceof Error &&
				failure.message === i18n.apiError('the current password is wrong');
		} finally {
			busy = false;
		}
	}
</script>

<svelte:head>
	<title>{t.pageTitle}</title>
</svelte:head>

<div class="cb-page-head">
	<h1 class="cb-page-title">{t.title}</h1>
</div>

<div class="stack">
	<section class="cb-panel">
		<div class="cb-panel-head">
			<h2 class="cb-label">{t.account}</h2>
		</div>
		<dl class="cb-kv-list">
			<div class="cb-kv">
				<dt>{t.username}</dt>
				<dd>{data.user?.username}</dd>
			</div>
		</dl>
	</section>

	<form class="cb-panel" onsubmit={changePassword}>
		<div class="cb-panel-head">
			<h2 class="cb-label">{t.password}</h2>
		</div>
		<div class="fields">
			{#key saveCount}
				<div class="cb-field">
					<label class="cb-label" for="current-password">{t.currentPassword}</label>
					<SecretInput
						id="current-password"
						bind:value={currentPassword}
						autocomplete="current-password"
						invalid={error !== '' && errorOnCurrent}
						describedBy={error && errorOnCurrent ? 'current-password-error' : undefined}
					/>
					{#if error && errorOnCurrent}
						<span class="cb-field-error" id="current-password-error" role="alert">{error}</span>
					{/if}
				</div>
				<div class="cb-field">
					<label class="cb-label" for="new-password">{t.newPassword}</label>
					<SecretInput
						id="new-password"
						bind:value={newPassword}
						autocomplete="new-password"
						invalid={error !== '' && !errorOnCurrent}
						describedBy="new-password-note"
					/>
					{#if error && !errorOnCurrent}
						<span class="cb-field-error" id="new-password-note" role="alert">{error}</span>
					{:else}
						<span class="cb-field-help" id="new-password-note">{t.newPasswordHelp}</span>
					{/if}
				</div>
			{/key}
		</div>
		<div class="cb-panel-foot">
			<Button type="submit" variant="primary" {busy}>
				{busy ? t.savingPassword : t.savePassword}
			</Button>
		</div>
	</form>

	<section class="cb-panel">
		<div class="cb-panel-head">
			<h2 class="cb-label">{t.appearance}</h2>
		</div>
		<div class="fields">
			<div class="cb-field">
				<span class="cb-label">{t.theme}</span>
				<div>
					<SegmentedControl
						label={t.theme}
						name="theme"
						options={themeOptions}
						bind:value={
							() => theme,
							(choice) => {
								theme = choice;
								applyTheme(choice);
							}
						}
					/>
				</div>
				<span class="cb-field-help">{t.themeHelp}</span>
			</div>
			<div class="cb-field">
				<span class="cb-label">{t.language}</span>
				<div>
					<SegmentedControl
						label={t.language}
						name="language"
						options={LOCALES}
						bind:value={() => i18n.locale, (locale) => i18n.setLocale(locale)}
					/>
				</div>
				<span class="cb-field-help">{t.languageHelp}</span>
			</div>
		</div>
	</section>
</div>

<style>
	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--space-6);
		max-width: 720px;
	}
	.fields {
		display: flex;
		flex-direction: column;
		gap: var(--space-5);
		padding: var(--space-5);
	}
</style>

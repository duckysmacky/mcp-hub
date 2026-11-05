<script lang="ts">
	import { api, messageOf } from '#lib/api/client.ts';
	import type { EnvStatus } from '#lib/api/types.ts';
	import Button from '#lib/components/Button.svelte';
	import ConfirmDialog from '#lib/components/ConfirmDialog.svelte';
	import SecretInput from '#lib/components/SecretInput.svelte';
	import Select from '#lib/components/Select.svelte';
	import { toasts } from '#lib/toasts.svelte.ts';
	import { tick } from 'svelte';

	let {
		instanceId,
		slug,
		env,
		allowedVars,
		liveProcesses,
		onchange,
		onrestart
	}: {
		instanceId: number;
		slug: string;
		env: EnvStatus[];
		allowedVars: string[];
		liveProcesses: number;
		onchange: () => Promise<void>;
		onrestart: () => void;
	} = $props();

	const VALUE_INPUT_ID = 'env-value';
	const ERROR_ID = 'env-error';

	const unsetVars = $derived(
		allowedVars.filter((name) => !env.some((entry) => entry.var === name))
	);
	const staleEntries = $derived(env.filter((entry) => !entry.in_catalog));
	const entryOf = (name: string) => env.find((entry) => entry.var === name);

	// Replace puts a variable that's already set back in the dropdown, for one edit.
	let replacing = $state<string | null>(null);
	const choices = $derived(replacing ? [replacing, ...unsetVars] : unsetVars);
	let selected = $derived(replacing ?? unsetVars[0] ?? '');

	let value = $state('');
	let error = $state('');
	let busy = $state(false);
	let removing = $state<string | null>(null);
	// Remounts the input after a save, so a value revealed with Show doesn't leave it unmasked.
	let saveCount = $state(0);

	async function focusValueInput() {
		error = '';
		// The form may only appear once this choice has rendered.
		await tick();
		document.getElementById(VALUE_INPUT_ID)?.focus();
	}

	function replace(name: string) {
		replacing = name;
		focusValueInput();
	}

	function set(name: string) {
		replacing = null;
		selected = name;
		focusValueInput();
	}

	function cancelReplace() {
		replacing = null;
		value = '';
		error = '';
	}

	const pathOf = (name: string) => `/instances/${instanceId}/env/${encodeURIComponent(name)}`;

	async function save(event: SubmitEvent) {
		event.preventDefault();
		busy = true;
		error = '';
		const name = selected;
		try {
			await api.put(pathOf(name), { value });
			// Secrets don't outlive the request that saved them.
			value = '';
			replacing = null;
			saveCount += 1;
			await onchange();
			if (liveProcesses > 0) {
				toasts.show('info', `${name} saved. Restart to apply.`, {
					label: 'Restart',
					run: onrestart
				});
			} else {
				toasts.show('ok', `${name} saved`);
			}
		} catch (failure) {
			error = messageOf(failure);
		} finally {
			busy = false;
		}
	}

	async function remove(name: string) {
		removing = null;
		try {
			await api.delete(pathOf(name));
			if (replacing === name) {
				cancelReplace();
			}
			await onchange();
			toasts.show('ok', `${name} removed`);
		} catch (failure) {
			toasts.show('err', messageOf(failure));
		}
	}
</script>

<section class="cb-panel">
	<div class="cb-panel-head">
		<h2 class="cb-label">Environment variables</h2>
		<span class="cb-hint">write-only</span>
	</div>

	<div class="cb-rows">
		{#each allowedVars as name (name)}
			{@const entry = entryOf(name)}
			<div class="cb-row">
				<strong class="name">{name}</strong>
				{#if entry}
					<span class="state">
						<span class="cb-state cb-state--ok">SET</span> - {entry.updated_at.slice(0, 10)}
					</span>
					<span class="actions">
						<Button variant="ghost" size="sm" onclick={() => replace(name)}>Replace</Button>
						<Button variant="ghost-danger" size="sm" onclick={() => (removing = name)}>
							Remove
						</Button>
					</span>
				{:else}
					<span class="state"><span class="cb-state cb-state--muted">NOT SET</span></span>
					<span class="actions">
						<Button variant="ghost" size="sm" onclick={() => set(name)}>Set</Button>
					</span>
				{/if}
			</div>
		{/each}
		{#each staleEntries as entry (entry.var)}
			<div class="cb-row">
				<strong class="name stale"><s>{entry.var}</s></strong>
				<span class="state">
					<span class="cb-state cb-state--muted">NO LONGER USED</span> - set
					{entry.updated_at.slice(0, 10)}
				</span>
				<span class="actions">
					<Button variant="ghost-danger" size="sm" onclick={() => (removing = entry.var)}>
						Remove
					</Button>
				</span>
			</div>
		{/each}
	</div>

	{#if choices.length > 0}
		<form class="cb-panel-foot" onsubmit={save}>
			<div class="cb-field variable">
				<label class="cb-label" for="env-var">Variable</label>
				<Select id="env-var" bind:value={selected}>
					{#each choices as name (name)}
						<option value={name}>{name}</option>
					{/each}
				</Select>
			</div>
			<div class="cb-field secret">
				<label class="cb-label" for={VALUE_INPUT_ID}>Value</label>
				{#key saveCount}
					<SecretInput
						id={VALUE_INPUT_ID}
						bind:value
						autocomplete="new-password"
						invalid={error !== ''}
						describedBy={error ? ERROR_ID : undefined}
					/>
				{/key}
			</div>
			<span class="save-actions">
				<Button type="submit" variant="primary" {busy}>{busy ? 'Saving' : 'Save'}</Button>
				{#if replacing}
					<Button variant="ghost" onclick={cancelReplace}>Cancel</Button>
				{/if}
			</span>
			{#if error}
				<p class="cb-field-error note" id={ERROR_ID} role="alert">{error}</p>
			{/if}
			<p class="cb-field-help note">
				Values are encrypted at rest and never shown again. You can replace or remove them.
			</p>
		</form>
	{:else if allowedVars.length > 0}
		<p class="cb-panel-foot cb-field-help">Every variable is set. Use Replace to change one.</p>
	{/if}
</section>

{#if removing}
	{@const name = removing}
	<ConfirmDialog
		context={`Instance ${slug}`}
		title={`Remove ${name}?`}
		detail="Its value is deleted. Running processes keep their copy until they restart."
		action="Remove variable"
		onconfirm={() => remove(name)}
		oncancel={() => (removing = null)}
	/>
{/if}

<style>
	.name {
		flex: 1 1 200px;
		min-width: 0;
		overflow-wrap: anywhere;
	}
	.stale {
		color: var(--text-3);
		font-weight: 500;
	}
	.state {
		flex: 1 1 180px;
		font-size: 12px;
		color: var(--text-2);
	}
	/* A fixed width keeps the state column aligned between rows with different actions. */
	.actions {
		flex: 0 0 140px;
		justify-content: flex-end;
	}
	.actions,
	.save-actions {
		display: flex;
		gap: var(--space-1);
	}
	.variable {
		flex: 1 1 200px;
	}
	.secret {
		flex: 2 1 240px;
	}
	.note {
		flex: 1 1 100%;
	}
	@media (max-width: 639px) {
		.actions {
			flex-basis: 100%;
			justify-content: flex-start;
		}
	}
</style>

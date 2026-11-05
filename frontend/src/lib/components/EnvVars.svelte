<script lang="ts">
	import { api, messageOf } from '#lib/api/client.ts';
	import type { EnvSpec, EnvStatus } from '#lib/api/types.ts';
	import Button from '#lib/components/Button.svelte';
	import ConfirmDialog from '#lib/components/ConfirmDialog.svelte';
	import SecretInput from '#lib/components/SecretInput.svelte';
	import Select from '#lib/components/Select.svelte';
	import { i18n } from '#lib/i18n/index.svelte.ts';
	import { toasts } from '#lib/toasts.svelte.ts';
	import { tick } from 'svelte';

	let {
		instanceId,
		slug,
		env,
		specs,
		missingRequired,
		liveProcesses,
		onchange,
		onrestart
	}: {
		instanceId: number;
		slug: string;
		env: EnvStatus[];
		specs: EnvSpec[];
		missingRequired: string[];
		liveProcesses: number;
		onchange: () => Promise<void>;
		onrestart: () => void;
	} = $props();

	const VALUE_INPUT_ID = 'env-value';
	const ERROR_ID = 'env-error';

	const entryOf = (name: string) => env.find((entry) => entry.var === name);
	const conflictsOf = (spec: EnvSpec) => spec.conflicts.filter((name) => entryOf(name));
	// Set values the catalog can't describe: it dropped the variable, or the server is off.
	const orphans = $derived(env.filter((entry) => !specs.some((spec) => spec.name === entry.var)));

	const settable = $derived(
		specs.filter((spec) => !entryOf(spec.name) && conflictsOf(spec).length === 0)
	);
	// Editing puts a variable that's already set back in the dropdown, for one edit.
	let editing = $state<string | null>(null);
	const choices = $derived([
		...specs.filter((spec) => spec.name === editing),
		...settable.filter((spec) => spec.name !== editing)
	]);
	let selected = $derived(editing ?? settable[0]?.name ?? '');
	const selectedSpec = $derived(specs.find((spec) => spec.name === selected));

	function startingValue(spec: EnvSpec | undefined): string {
		const current = spec && entryOf(spec.name)?.value;
		if (current !== undefined) {
			return current;
		}
		if (spec?.type === 'boolean') {
			return 'true';
		}
		return spec?.type === 'enum' ? (spec.enum_values[0] ?? '') : '';
	}

	// Follows the chosen variable, until the user types.
	let value = $derived(startingValue(selectedSpec));
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

	function edit(name: string) {
		editing = name;
		focusValueInput();
	}

	function set(name: string) {
		editing = null;
		selected = name;
		focusValueInput();
	}

	function cancelEdit() {
		editing = null;
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
			editing = null;
			saveCount += 1;
			await onchange();
			if (liveProcesses > 0) {
				toasts.show('info', i18n.t.env.savedRestart(name), {
					label: i18n.t.env.restart,
					run: onrestart
				});
			} else {
				toasts.show('ok', i18n.t.env.saved(name));
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
			if (editing === name) {
				cancelEdit();
			}
			await onchange();
			toasts.show('ok', i18n.t.env.removed(name));
		} catch (failure) {
			toasts.show('err', messageOf(failure));
		}
	}
</script>

{#snippet removeButton(name: string)}
	<Button variant="ghost-danger" size="sm" onclick={() => (removing = name)}>
		{i18n.t.env.remove}
	</Button>
{/snippet}

{#snippet setState(entry: EnvStatus)}
	<span class="state">
		<span class="cb-state cb-state--ok">{i18n.t.env.stateSet}</span> - {entry.updated_at.slice(
			0,
			10
		)}
		{#if entry.value !== undefined}<code class="current">{entry.value}</code>{/if}
	</span>
{/snippet}

<section class="cb-panel">
	<div class="cb-panel-head">
		<h2 class="cb-label">{i18n.t.env.heading}</h2>
		<span class="cb-hint">{i18n.t.env.hint}</span>
	</div>

	<div class="cb-rows">
		{#each specs as spec (spec.name)}
			{@const entry = entryOf(spec.name)}
			{@const conflicts = conflictsOf(spec)}
			<div class={['cb-row', missingRequired.includes(spec.name) && 'cb-row--warn']}>
				<div class="name">
					<div>
						<strong>{spec.name}</strong>
						{#if spec.required && conflicts.length === 0}
							<span class={['cb-tag', !entry && 'cb-tag--warn']}>{i18n.t.common.required}</span>
						{/if}
					</div>
					{#if spec.description}<div class="cb-hint">{spec.description}</div>{/if}
					{#if spec.conflicts.length > 0 && conflicts.length === 0}
						<div class="cb-hint">{i18n.t.env.alternativeTo(spec.conflicts.join(', '))}</div>
					{/if}
				</div>
				{#if entry}
					{@render setState(entry)}
					<span class="actions">
						<Button variant="ghost" size="sm" onclick={() => edit(spec.name)}>
							{spec.secret ? i18n.t.env.replace : i18n.t.env.edit}
						</Button>
						{@render removeButton(spec.name)}
					</span>
				{:else}
					<span class="state">
						<span
							class={[
								'cb-state',
								missingRequired.includes(spec.name) ? 'cb-state--warn' : 'cb-state--muted'
							]}
						>
							{i18n.t.env.stateNotSet}
						</span>
						{#if conflicts.length > 0}
							<span class="cb-hint conflict">{i18n.t.env.conflictsWith(conflicts.join(', '))}</span>
						{/if}
					</span>
					<span class="actions">
						{#if conflicts.length === 0}
							<Button variant="ghost" size="sm" onclick={() => set(spec.name)}>
								{i18n.t.env.set}
							</Button>
						{/if}
					</span>
				{/if}
			</div>
		{/each}
		{#each orphans as entry (entry.var)}
			<div class="cb-row">
				{#if entry.in_catalog}
					<strong class="name">{entry.var}</strong>
					{@render setState(entry)}
				{:else}
					<strong class="name stale"><s>{entry.var}</s></strong>
					<span class="state">
						<span class="cb-state cb-state--muted">{i18n.t.env.stateUnused}</span> -
						{i18n.t.env.setOn(entry.updated_at.slice(0, 10))}
					</span>
				{/if}
				<span class="actions">{@render removeButton(entry.var)}</span>
			</div>
		{/each}
	</div>

	{#if selectedSpec}
		<form class="cb-panel-foot" onsubmit={save}>
			<div class="cb-field variable">
				<label class="cb-label" for="env-var">{i18n.t.env.variable}</label>
				<Select id="env-var" bind:value={() => selected, (name) => (selected = name)}>
					{#each choices as spec (spec.name)}
						<option value={spec.name}>{spec.name}</option>
					{/each}
				</Select>
			</div>
			<div class="cb-field entry">
				<label class="cb-label" for={VALUE_INPUT_ID}>{i18n.t.env.value}</label>
				{#if selectedSpec.secret}
					{#key saveCount}
						<SecretInput
							id={VALUE_INPUT_ID}
							bind:value
							autocomplete="new-password"
							invalid={error !== ''}
							describedBy={error ? ERROR_ID : undefined}
						/>
					{/key}
				{:else if selectedSpec.type === 'boolean' || selectedSpec.type === 'enum'}
					<Select id={VALUE_INPUT_ID} bind:value>
						{#each selectedSpec.type === 'boolean' ? ['true', 'false'] : selectedSpec.enum_values as option (option)}
							<option value={option}>{option}</option>
						{/each}
					</Select>
				{:else}
					<input
						class="cb-input"
						id={VALUE_INPUT_ID}
						type={selectedSpec.type === 'number' ? 'number' : 'text'}
						step={selectedSpec.type === 'number' ? 'any' : undefined}
						bind:value={() => value, (typed) => (value = String(typed ?? ''))}
						autocomplete="off"
						autocapitalize="none"
						spellcheck="false"
						required={selectedSpec.required}
						aria-invalid={error ? true : undefined}
						aria-describedby={error ? ERROR_ID : undefined}
					/>
				{/if}
			</div>
			<span class="save-actions">
				<Button type="submit" variant="primary" {busy}>
					{busy ? i18n.t.env.saving : i18n.t.env.save}
				</Button>
				{#if editing}
					<Button variant="ghost" onclick={cancelEdit}>{i18n.t.common.cancel}</Button>
				{/if}
			</span>
			{#if error}
				<p class="cb-field-error note" id={ERROR_ID} role="alert">{error}</p>
			{/if}
			<p class="cb-field-help note">
				{selectedSpec.secret ? i18n.t.env.secretHelp : i18n.t.env.readableHelp}
			</p>
		</form>
	{:else if specs.length > 0}
		<p class="cb-panel-foot cb-field-help">{i18n.t.env.allSet}</p>
	{/if}
</section>

{#if removing}
	{@const name = removing}
	<ConfirmDialog
		context={i18n.t.common.instanceContext(slug)}
		title={i18n.t.env.removeTitle(name)}
		detail={i18n.t.env.removeDetail}
		action={i18n.t.env.removeAction}
		onconfirm={() => remove(name)}
		oncancel={() => (removing = null)}
	/>
{/if}

<style>
	.name {
		flex: 1 1 240px;
		min-width: 0;
		overflow-wrap: anywhere;
	}
	.stale {
		color: var(--text-3);
		font-weight: 500;
	}
	.state {
		flex: 1 1 180px;
		min-width: 0;
		font-size: 12px;
		color: var(--text-2);
	}
	.current {
		display: block;
		color: var(--text);
		overflow-wrap: anywhere;
	}
	.conflict {
		display: block;
	}
	/* A fixed width keeps the state column aligned between rows with different actions. */
	.actions {
		flex: 0 0 160px;
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
	.entry {
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

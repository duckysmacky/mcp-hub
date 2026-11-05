<script lang="ts">
	import { messageOf } from '#lib/api/client.ts';
	import type { EnvSpec, EnvStatus } from '#lib/api/types.ts';
	import Button from '#lib/components/Button.svelte';
	import SecretInput from '#lib/components/SecretInput.svelte';
	import Select from '#lib/components/Select.svelte';
	import { i18n } from '#lib/i18n/index.svelte.ts';
	import { tick } from 'svelte';

	let {
		spec,
		entry,
		savedValue,
		conflicts,
		isMissing,
		onsave,
		onremove
	}: {
		spec: EnvSpec;
		entry: EnvStatus | undefined;
		/** Apart from `entry`, so reloading after another row's save keeps this row's typing. */
		savedValue: string;
		conflicts: string[];
		isMissing: boolean;
		onsave: (name: string, value: string) => Promise<void>;
		onremove: (name: string) => void;
	} = $props();

	const t = $derived(i18n.t.env);
	const inputId = $derived(`env-${spec.name}`);
	const errorId = $derived(`env-${spec.name}-error`);

	// Follows the saved value, until the user types.
	let value = $derived(savedValue);
	let replacing = $state(false);
	let error = $state('');
	let busy = $state(false);
	// Remounts the input after a save, so a value revealed with Show doesn't leave it unmasked.
	let saveCount = $state(0);

	const isSet = $derived(entry !== undefined);
	// A saved secret can only be replaced, so it hides behind a placeholder until asked.
	const showsMask = $derived(spec.secret && isSet && !replacing);
	const canSave = $derived(value !== '' && value !== savedValue);

	async function save(event: SubmitEvent) {
		event.preventDefault();
		if (!canSave || busy) {
			return;
		}
		busy = true;
		error = '';
		try {
			await onsave(spec.name, value);
			if (spec.secret) {
				// Secrets don't outlive the request that saved them.
				value = '';
				saveCount += 1;
			}
			replacing = false;
		} catch (failure) {
			error = messageOf(failure);
		} finally {
			busy = false;
		}
	}

	async function startReplacing() {
		replacing = true;
		error = '';
		await tick();
		document.getElementById(inputId)?.focus();
	}

	function cancelReplacing() {
		replacing = false;
		value = '';
		error = '';
	}
</script>

<div class={['cb-row', 'env-row', isMissing && 'cb-row--warn']}>
	<div class="about">
		<div class="title">
			<label class="name" for={inputId}>{spec.name}</label>
			{#if spec.secret}
				<span class="cb-tag secret-tag">
					<svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
						<path
							d="M2.75 5.75h6.5v4.5h-6.5zM4 5.75V3.5a2 2 0 0 1 4 0v2.25"
							fill="none"
							stroke="currentColor"
							stroke-width="1.5"
							stroke-linecap="square"
						/>
					</svg>
					{t.secretTag}
				</span>
			{/if}
		</div>
		{#if spec.description}<div class="cb-hint">{spec.description}</div>{/if}
		{#if spec.conflicts.length > 0 && conflicts.length === 0}
			<div class="cb-hint">{t.alternativeTo(spec.conflicts.join(', '))}</div>
		{/if}
	</div>

	<form class="control" onsubmit={save}>
		{#if conflicts.length > 0}
			<p class="cb-hint">{t.conflictsWith(conflicts.join(', '))}</p>
		{:else}
			<div class="inputs">
				<div class="field">
					{#if showsMask}
						<div class="cb-input mask" aria-hidden="true">••••••••</div>
					{:else if spec.secret}
						{#key saveCount}
							<SecretInput
								id={inputId}
								bind:value
								autocomplete="new-password"
								placeholder={t.secretPlaceholder}
								invalid={error !== ''}
								describedBy={error ? errorId : undefined}
							/>
						{/key}
					{:else if spec.type === 'boolean' || spec.type === 'enum'}
						<Select id={inputId} bind:value>
							{#if !isSet}<option value="">{t.notSetOption}</option>{/if}
							{#each spec.type === 'boolean' ? ['true', 'false'] : spec.enum_values as option (option)}
								<option value={option}>{option}</option>
							{/each}
						</Select>
					{:else}
						<input
							class="cb-input"
							id={inputId}
							type={spec.type === 'number' ? 'number' : 'text'}
							step={spec.type === 'number' ? 'any' : undefined}
							bind:value={() => value, (typed) => (value = String(typed ?? ''))}
							autocomplete="off"
							autocapitalize="none"
							spellcheck="false"
							aria-invalid={error ? true : undefined}
							aria-describedby={error ? errorId : undefined}
						/>
					{/if}
				</div>
				<div class="actions">
					{#if canSave}
						<Button type="submit" {busy}>{busy ? t.saving : t.save}</Button>
					{/if}
					{#if showsMask}
						<Button onclick={startReplacing}>{t.replace}</Button>
					{:else if replacing}
						<Button variant="ghost" onclick={cancelReplacing}>{i18n.t.common.cancel}</Button>
					{/if}
					{#if isSet && !replacing}
						<Button variant="ghost-danger" onclick={() => onremove(spec.name)}>{t.remove}</Button>
					{/if}
				</div>
			</div>
			{#if error}
				<p class="cb-field-error" id={errorId} role="alert">{error}</p>
			{:else if entry}
				<p class="state">
					<span class="cb-state cb-state--ok">{t.stateSet}</span> - {entry.updated_at.slice(0, 10)}
				</p>
			{:else}
				<p class="state">
					<span class={['cb-state', isMissing ? 'cb-state--warn' : 'cb-state--muted']}>
						{t.stateNotSet}
					</span>
				</p>
			{/if}
		{/if}
	</form>
</div>

<style>
	.env-row {
		align-items: flex-start;
		gap: var(--space-3) var(--space-5);
		padding-block: var(--space-3);
	}
	.about {
		flex: 1 1 240px;
		min-width: 0;
		overflow-wrap: anywhere;
	}
	.title {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--space-1) var(--space-2);
	}
	.name {
		font-weight: 600;
	}
	.secret-tag {
		gap: var(--space-1);
	}
	.control {
		flex: 1 1 340px;
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
	}
	.inputs {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-2);
	}
	.field {
		flex: 1 1 180px;
		min-width: 0;
	}
	.actions {
		display: flex;
		gap: var(--space-1);
	}
	.mask {
		display: flex;
		align-items: center;
		background: var(--code-bg);
		border-color: var(--border-strong);
		color: var(--text-2);
		letter-spacing: 0.1em;
	}
	.state {
		font-size: 11px;
		line-height: 16px;
		color: var(--text-2);
	}
</style>

<script lang="ts">
	import type { EnvSpec, Runtime, ServerMode } from '#lib/api/types.ts';
	import Button from '#lib/components/Button.svelte';
	import Select from '#lib/components/Select.svelte';
	import { emptyEnvSpec, type CustomServerDraft } from '#lib/custom-server.ts';
	import { i18n } from '#lib/i18n/index.svelte.ts';

	const RUNTIMES: Runtime[] = ['bun', 'node', 'uv', 'pip', 'binary'];
	const MODES: ServerMode[] = ['stdio', 'http'];
	const TYPES: EnvSpec['type'][] = ['string', 'number', 'boolean', 'enum'];

	let {
		idPrefix,
		draft = $bindable()
	}: {
		idPrefix: string;
		draft: CustomServerDraft;
	} = $props();

	const t = $derived(i18n.t.custom);
	const isBinary = $derived(draft.runtime === 'binary');
	const isPython = $derived(draft.runtime === 'uv' || draft.runtime === 'pip');
	const id = (name: string) => `${idPrefix}-${name}`;
</script>

<div class="fields">
	<div class="pair">
		<div class="cb-field">
			<label class="cb-label" for={id('runtime')}>{t.runtime}</label>
			<Select id={id('runtime')} bind:value={draft.runtime}>
				{#each RUNTIMES as runtime (runtime)}
					<option value={runtime}>{runtime}</option>
				{/each}
			</Select>
		</div>
		<div class="cb-field">
			<label class="cb-label" for={id('mode')}>{t.mode}</label>
			<Select id={id('mode')} bind:value={draft.mode}>
				{#each MODES as mode (mode)}
					<option value={mode}>{mode}</option>
				{/each}
			</Select>
		</div>
	</div>

	<div class="cb-field">
		<label class="cb-label" for={id('package')}>{isBinary ? t.binaryPath : t.package}</label>
		<input
			class="cb-input"
			id={id('package')}
			bind:value={draft.package}
			placeholder={isBinary ? '/opt/mcp/my-server' : 'name@1.2.3'}
			autocapitalize="none"
			autocomplete="off"
			spellcheck="false"
			required
			aria-describedby={id('package-note')}
		/>
		<span class="cb-field-help" id={id('package-note')}>
			{isBinary ? t.binaryPathHelp : t.packageHelp}
		</span>
	</div>

	{#if !isBinary}
		<div class="cb-field">
			<label class="cb-label" for={id('bin')}>{t.bin}</label>
			<input
				class="cb-input"
				id={id('bin')}
				bind:value={draft.bin}
				autocapitalize="none"
				autocomplete="off"
				spellcheck="false"
				required={isPython}
				aria-describedby={id('bin-note')}
			/>
			<span class="cb-field-help" id={id('bin-note')}>{isPython ? t.binPython : t.binHelp}</span>
		</div>
	{/if}

	<div class="cb-field">
		<label class="cb-label" for={id('args')}>{t.args}</label>
		<textarea
			class="cb-input text"
			id={id('args')}
			bind:value={draft.args}
			rows="3"
			autocapitalize="none"
			autocomplete="off"
			spellcheck="false"
			aria-describedby={id('args-note')}></textarea>
		<span class="cb-field-help" id={id('args-note')}>{t.argsHelp}</span>
	</div>

	{#if draft.mode === 'http'}
		<div class="pair">
			<div class="cb-field">
				<label class="cb-label" for={id('port-env')}>{t.portEnv}</label>
				<input
					class="cb-input"
					id={id('port-env')}
					bind:value={draft.portEnv}
					autocapitalize="none"
					autocomplete="off"
					spellcheck="false"
					required
				/>
			</div>
			<div class="cb-field">
				<label class="cb-label" for={id('path')}>{t.path}</label>
				<input
					class="cb-input"
					id={id('path')}
					bind:value={draft.path}
					autocapitalize="none"
					autocomplete="off"
					spellcheck="false"
					required
				/>
			</div>
		</div>
		<div class="cb-field">
			<label class="cb-label" for={id('headers')}>{t.headers}</label>
			<textarea
				class="cb-input text"
				id={id('headers')}
				bind:value={draft.headers}
				rows="2"
				placeholder={'Authorization: Bearer {MY_TOKEN}'}
				autocapitalize="none"
				autocomplete="off"
				spellcheck="false"
				aria-describedby={id('headers-note')}></textarea>
			<span class="cb-field-help" id={id('headers-note')}>{t.headersHelp}</span>
		</div>
	{/if}

	<div class="cb-field">
		<span class="cb-label">{t.variables}</span>
		<span class="cb-field-help">{t.variablesHelp}</span>
		{#each draft.env as spec, index (index)}
			<fieldset class="variable">
				<legend class="cb-label">{spec.name || t.newVariable}</legend>
				<div class="pair">
					<div class="cb-field">
						<label class="cb-label" for={id(`env-name-${index}`)}>{t.varName}</label>
						<input
							class="cb-input"
							id={id(`env-name-${index}`)}
							bind:value={spec.name}
							autocapitalize="none"
							autocomplete="off"
							spellcheck="false"
							required
						/>
					</div>
					<div class="cb-field">
						<label class="cb-label" for={id(`env-type-${index}`)}>{t.varType}</label>
						<Select id={id(`env-type-${index}`)} bind:value={spec.type}>
							{#each TYPES as type (type)}
								<option value={type}>{type}</option>
							{/each}
						</Select>
					</div>
				</div>
				{#if spec.type === 'enum'}
					<div class="cb-field">
						<label class="cb-label" for={id(`env-enum-${index}`)}>{t.varEnum}</label>
						<input
							class="cb-input"
							id={id(`env-enum-${index}`)}
							bind:value={spec.enumText}
							autocomplete="off"
							spellcheck="false"
							required
						/>
					</div>
				{/if}
				<div class="cb-field">
					<label class="cb-label" for={id(`env-description-${index}`)}>{t.varDescription}</label>
					<input
						class="cb-input"
						id={id(`env-description-${index}`)}
						bind:value={spec.description}
						autocomplete="off"
					/>
				</div>
				<div class="switches">
					<button
						class="cb-toggle"
						type="button"
						role="switch"
						aria-checked={spec.secret}
						onclick={() => (spec.secret = !spec.secret)}
					>
						<span class="cb-toggle-track"><span class="cb-toggle-thumb"></span></span>
						<span class="cb-toggle-text">{t.varSecret}</span>
					</button>
					<button
						class="cb-toggle"
						type="button"
						role="switch"
						aria-checked={spec.required}
						onclick={() => (spec.required = !spec.required)}
					>
						<span class="cb-toggle-track"><span class="cb-toggle-thumb"></span></span>
						<span class="cb-toggle-text">{t.varRequired}</span>
					</button>
					<Button variant="ghost-danger" size="sm" onclick={() => draft.env.splice(index, 1)}>
						{t.removeVariable}
					</Button>
				</div>
			</fieldset>
		{/each}
		<div>
			<Button size="sm" onclick={() => draft.env.push(emptyEnvSpec())}>{t.addVariable}</Button>
		</div>
	</div>
</div>

<style>
	.fields {
		display: flex;
		flex-direction: column;
		gap: var(--space-5);
	}
	.pair {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(160px, 1fr));
		gap: var(--space-4);
	}
	.text {
		height: auto;
		padding-top: var(--space-2);
		padding-bottom: var(--space-2);
		resize: vertical;
	}
	.variable {
		display: flex;
		flex-direction: column;
		gap: var(--space-4);
		min-width: 0;
		margin: var(--space-2) 0;
		padding: var(--space-4);
		border: var(--bw) solid var(--border);
	}
	.switches {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--space-4);
	}
	/* Cobalt sizes this for ON and OFF; these are longer words. */
	.switches .cb-toggle-text {
		width: auto;
	}
</style>

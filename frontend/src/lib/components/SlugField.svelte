<script lang="ts">
	import { i18n } from '#lib/i18n/index.svelte.ts';

	let {
		id,
		value = $bindable(''),
		error = ''
	}: { id: string; value?: string; error?: string } = $props();

	// Mirrors is_valid_slug in the backend.
	const SLUG = /^[a-z0-9][a-z0-9-]*$/;

	const message = $derived(error || (value && !SLUG.test(value) ? i18n.t.slug.invalid : ''));
</script>

<div class="cb-field">
	<label class="cb-label" for={id}>{i18n.t.slug.label}</label>
	<div class="cb-input--prefixed">
		<span class="cb-prefix">/mcp/</span>
		<input
			class="cb-input"
			{id}
			bind:value={() => value, (typed) => (value = typed.toLowerCase())}
			autocapitalize="none"
			autocomplete="off"
			spellcheck="false"
			maxlength="63"
			pattern="[a-z0-9][a-z0-9\-]*"
			required
			aria-invalid={message ? true : undefined}
			aria-describedby={`${id}-note`}
		/>
	</div>
	{#if message}
		<span class="cb-field-error" id={`${id}-note`}>{message}</span>
	{:else}
		<span class="cb-field-help" id={`${id}-note`}>
			{i18n.t.slug.help}
		</span>
	{/if}
</div>

<script lang="ts">
	let {
		id,
		value = $bindable(''),
		error = ''
	}: { id: string; value?: string; error?: string } = $props();

	// Mirrors is_valid_slug in the backend.
	const SLUG = /^[a-z0-9][a-z0-9-]*$/;

	const message = $derived(
		error ||
			(value && !SLUG.test(value)
				? 'Use lowercase letters, digits and dashes, starting with a letter or digit.'
				: '')
	);
</script>

<div class="cb-field">
	<label class="cb-label" for={id}>Instance slug</label>
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
			Part of your connection URL. Lowercase letters, digits and dashes.
		</span>
	{/if}
</div>

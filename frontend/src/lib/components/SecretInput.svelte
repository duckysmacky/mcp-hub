<script lang="ts">
	import { i18n } from '#lib/i18n/index.svelte.ts';

	let {
		id,
		value = $bindable(''),
		autocomplete,
		required = true,
		describedBy,
		invalid = false,
		placeholder
	}: {
		id: string;
		value?: string;
		autocomplete: 'current-password' | 'new-password';
		required?: boolean;
		describedBy?: string;
		invalid?: boolean;
		placeholder?: string;
	} = $props();

	let shown = $state(false);
</script>

<div class="cb-secret">
	<input
		class="cb-input"
		{id}
		type={shown ? 'text' : 'password'}
		bind:value
		{autocomplete}
		{required}
		{placeholder}
		autocapitalize="none"
		spellcheck="false"
		aria-invalid={invalid || undefined}
		aria-describedby={describedBy}
	/>
	<button
		class="cb-btn cb-btn--ghost cb-btn--sm"
		type="button"
		aria-controls={id}
		aria-pressed={shown}
		onclick={() => (shown = !shown)}
	>
		{shown ? i18n.t.common.hide : i18n.t.common.show}
	</button>
</div>

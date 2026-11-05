<script lang="ts">
	import type { CustomServer } from '#lib/api/types.ts';
	import Button from '#lib/components/Button.svelte';
	import CustomServerForm from '#lib/components/CustomServerForm.svelte';
	import { definitionOf, draftOf } from '#lib/custom-server.ts';
	import { i18n } from '#lib/i18n/index.svelte.ts';
	import { untrack } from 'svelte';

	let {
		custom,
		busy,
		onsave
	}: {
		custom: CustomServer;
		busy: boolean;
		onsave: (definition: CustomServer) => void;
	} = $props();

	// The page re-creates this panel when the saved definition changes.
	let draft = $state(draftOf(untrack(() => custom)));

	function submit(event: SubmitEvent) {
		event.preventDefault();
		onsave(definitionOf($state.snapshot(draft)));
	}
</script>

<form class="cb-panel" onsubmit={submit}>
	<div class="cb-panel-head">
		<h2 class="cb-label">{i18n.t.custom.definition}</h2>
	</div>
	<div class="cb-panel-body">
		<CustomServerForm idPrefix="definition" bind:draft />
	</div>
	<div class="cb-panel-foot">
		<Button type="submit" {busy}>{i18n.t.custom.save}</Button>
	</div>
</form>

<script lang="ts" generics="Option extends string">
	let {
		label,
		name,
		options,
		value = $bindable()
	}: {
		label: string;
		name: string;
		options: readonly { value: Option; label: string }[];
		value: Option;
	} = $props();
</script>

<div class="cb-seg" role="radiogroup" aria-label={label}>
	{#each options as option (option.value)}
		<label class="cb-seg-opt">
			<input type="radio" {name} value={option.value} bind:group={value} />
			<span>{option.label}</span>
		</label>
	{/each}
</div>

<script lang="ts">
	import type { StatusLabel } from '#lib/status.ts';

	let { status }: { status: StatusLabel } = $props();
</script>

<span class={['cb-badge', `cb-badge--${status.tone}`]}>[ {status.word} ]</span>

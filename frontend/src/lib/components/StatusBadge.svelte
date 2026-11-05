<script lang="ts">
	import { i18n } from '#lib/i18n/index.svelte.ts';
	import type { StatusLabel } from '#lib/status.ts';

	let { status }: { status: StatusLabel } = $props();
</script>

<span class={['cb-badge', `cb-badge--${status.tone}`]}>[ {i18n.t.status[status.word]} ]</span>

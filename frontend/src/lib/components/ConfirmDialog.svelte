<script lang="ts">
	import Button from '#lib/components/Button.svelte';

	let {
		context,
		title,
		detail,
		action,
		variant = 'danger',
		onconfirm,
		oncancel
	}: {
		context: string;
		title: string;
		detail: string;
		action: string;
		variant?: 'danger' | 'secondary';
		onconfirm: () => void;
		oncancel: () => void;
	} = $props();

	const IDS = { title: 'confirm-title', detail: 'confirm-detail' };
</script>

<!--
	Mounted only while needed. The native dialog traps focus, handles Escape, and focuses Cancel
	because it comes first; close() on the way out returns focus to whatever opened it.
-->
<dialog
	class="cb-dialog"
	role="alertdialog"
	aria-modal="true"
	aria-labelledby={IDS.title}
	aria-describedby={IDS.detail}
	{oncancel}
	onclick={(event) => {
		// Only a click on the backdrop lands on the dialog itself: its contents cover the rest.
		if (event.target === event.currentTarget) {
			oncancel();
		}
	}}
	{@attach (dialog) => {
		dialog.showModal();
		return () => dialog.close();
	}}
>
	<div class="cb-dialog-head">
		<span class="cb-label">{context}</span>
		<h2 class="cb-dialog-title" id={IDS.title}>{title}</h2>
	</div>
	<div class="cb-dialog-body" id={IDS.detail}><p>{detail}</p></div>
	<div class="cb-dialog-foot">
		<Button variant="ghost" onclick={oncancel}>Cancel</Button>
		<Button {variant} onclick={onconfirm}>{action}</Button>
	</div>
</dialog>

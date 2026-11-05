export interface ToastAction {
	label: string;
	run: () => void;
}

export interface Toast {
	id: number;
	tone: 'ok' | 'info' | 'err';
	message: string;
	action?: ToastAction;
}

const MAX_VISIBLE = 3;
const AUTO_DISMISS_MS = 4000;

class Toasts {
	items = $state.raw<Toast[]>([]);
	#nextId = 1;
	#timers = new Map<number, ReturnType<typeof setTimeout>>();

	show(tone: Toast['tone'], message: string, action?: ToastAction) {
		const toast: Toast = { id: this.#nextId++, tone, message, action };
		this.items = [...this.items, toast].slice(-MAX_VISIBLE);
		this.resume(toast);
	}

	dismiss(id: number) {
		this.pause(id);
		this.items = this.items.filter((toast) => toast.id !== id);
	}

	pause(id: number) {
		clearTimeout(this.#timers.get(id));
		this.#timers.delete(id);
	}

	/** Errors stay until dismissed, so they can't vanish unread. */
	resume(toast: Toast) {
		if (toast.tone !== 'err') {
			this.#timers.set(
				toast.id,
				setTimeout(() => this.dismiss(toast.id), AUTO_DISMISS_MS)
			);
		}
	}
}

export const toasts = new Toasts();

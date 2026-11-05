export type ThemeChoice = 'system' | 'dark' | 'light';

const STORAGE_KEY = 'theme';

export function storedTheme(): ThemeChoice {
	try {
		const saved = localStorage.getItem(STORAGE_KEY);
		return saved === 'dark' || saved === 'light' ? saved : 'system';
	} catch {
		return 'system';
	}
}

/** Mirrors the script in app.html, which applies the choice before first paint. */
export function applyTheme(choice: ThemeChoice) {
	const prefersLight = matchMedia('(prefers-color-scheme: light)').matches;
	const resolved = choice === 'system' ? (prefersLight ? 'light' : 'dark') : choice;
	document.documentElement.dataset.theme = resolved;
	try {
		if (choice === 'system') {
			localStorage.removeItem(STORAGE_KEY);
		} else {
			localStorage.setItem(STORAGE_KEY, choice);
		}
	} catch {
		// Storage can be blocked: the choice just lasts until the next reload.
	}
}

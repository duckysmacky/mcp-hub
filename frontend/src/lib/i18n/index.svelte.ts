import { browser } from '$app/env';
import { localizeApiError } from './api-errors.ts';
import { en } from './en.ts';
import { ru } from './ru.ts';

const MESSAGES = { en, ru };

export type Locale = keyof typeof MESSAGES;

export const LOCALES: readonly { value: Locale; label: string }[] = [
	{ value: 'en', label: 'English' },
	{ value: 'ru', label: 'Русский' }
];

class I18n {
	// app.html picks the language before first paint.
	locale = $state<Locale>(browser && document.documentElement.lang === 'ru' ? 'ru' : 'en');

	get t() {
		return MESSAGES[this.locale];
	}

	setLocale(locale: Locale) {
		this.locale = locale;
		document.documentElement.lang = locale;
		try {
			localStorage.setItem('language', locale);
		} catch {
			// Storage can be blocked: the choice just lasts until the next reload.
		}
	}

	apiError(message: string) {
		return localizeApiError(message, this.t.apiErrors);
	}
}

export const i18n = new I18n();

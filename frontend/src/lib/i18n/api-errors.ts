export type ApiErrorRule = [pattern: RegExp, translate: (match: RegExpMatchArray) => string];

/**
 * The backend has no error codes, only English sentences, which it writes mid-sentence:
 * "slug must be lowercase...". An unknown one still reads fine, capitalized.
 */
export function localizeApiError(message: string, rules: ApiErrorRule[]): string {
	for (const [pattern, translate] of rules) {
		const match = message.match(pattern);
		if (match) {
			return translate(match);
		}
	}
	return message.charAt(0).toUpperCase() + message.slice(1);
}

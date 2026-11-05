const FALLBACK = '/instances';

/**
 * Where to go after login. `next` comes from the URL, so anyone can craft it: only a
 * path on this site is accepted, which stops the login page being an open redirect.
 */
export function safeNextPath(next: string | null): string {
	if (!next || !next.startsWith('/') || next.startsWith('//') || next.includes('\\')) {
		return FALLBACK;
	}
	// Browsers strip tabs and newlines from URLs, so `/\t/evil.example` would become `//evil.example`.
	// eslint-disable-next-line no-control-regex
	if (/[\u0000-\u001f]/.test(next)) {
		return FALLBACK;
	}
	return next;
}

const UNITS: [Intl.RelativeTimeFormatUnit, number][] = [
	['day', 86400],
	['hour', 3600],
	['minute', 60]
];

/** "4 minutes ago" for recent activity, a plain date past a week, per Cobalt's voice. */
export function relativeTime(iso: string, locale: string, now = Date.now()): string {
	const seconds = Math.max(0, (now - new Date(iso).getTime()) / 1000);
	if (seconds >= 7 * 86400) {
		return iso.slice(0, 10);
	}
	const format = new Intl.RelativeTimeFormat(locale, { style: 'short' });
	for (const [unit, length] of UNITS) {
		if (seconds >= length) {
			return format.format(-Math.floor(seconds / length), unit);
		}
	}
	// -0, or it reads "in 0 min.".
	return format.format(-0, 'minute');
}

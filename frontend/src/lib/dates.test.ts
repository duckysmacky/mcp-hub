import { expect, test } from 'vitest';
import { relativeTime } from './dates';

const NOW = Date.parse('2026-11-05T12:00:00Z');

test('recent times are relative, in the chosen language', () => {
	expect(relativeTime('2026-11-05T11:56:00Z', 'en', NOW)).toBe('4 min. ago');
	expect(relativeTime('2026-11-05T09:00:00Z', 'en', NOW)).toBe('3 hr. ago');
	expect(relativeTime('2026-11-05T11:56:00Z', 'ru', NOW)).toBe('4 мин. назад');
});

test('just now is in the past', () => {
	expect(relativeTime('2026-11-05T11:59:50Z', 'en', NOW)).toBe('0 min. ago');
});

test('old times are plain dates', () => {
	expect(relativeTime('2026-10-01T11:56:00Z', 'en', NOW)).toBe('2026-10-01');
});

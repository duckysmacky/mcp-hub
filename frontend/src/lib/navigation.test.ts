import { expect, test } from 'vitest';
import { safeNextPath } from './navigation';

test('keeps paths on this site', () => {
	expect(safeNextPath('/instances/5')).toBe('/instances/5');
	expect(safeNextPath('/consent?request=abc')).toBe('/consent?request=abc');
});

test('rejects anything that could leave this site', () => {
	for (const next of [
		null,
		'',
		'instances',
		'https://evil.example',
		'//evil.example',
		'/\\evil.example',
		'\\\\evil.example',
		'/\t/evil.example',
		'/\n/evil.example',
		'javascript:alert(1)'
	]) {
		expect(safeNextPath(next)).toBe('/instances');
	}
});

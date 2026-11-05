import { expect, test } from 'vitest';
import { clientGuides } from './clients';

const URL = 'https://mcp.example.com/mcp/taiga';

test('every guide reaches the instance and has steps', () => {
	for (const guide of clientGuides('taiga', URL)) {
		const copies = guide.steps.flatMap((step) => step.copy ?? []);
		expect(copies.join(' '), guide.name).toContain(URL);
	}
});

test('config snippets are valid JSON keyed by the slug', () => {
	const snippets = clientGuides('taiga', URL)
		.flatMap((guide) => guide.steps)
		.flatMap((step) => (step.copy?.startsWith('{') ? [step.copy] : []));

	expect(snippets.length).toBeGreaterThan(0);
	for (const snippet of snippets) {
		const [servers] = Object.values<Record<string, { url: string }>>(JSON.parse(snippet));
		expect(servers.taiga.url).toBe(URL);
	}
});

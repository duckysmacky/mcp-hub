import { describe, expect, it } from 'vitest';
import { definitionOf, draftOf, emptyDraft, emptyEnvSpec } from './custom-server.ts';

describe('custom server drafts', () => {
	it('turns text fields into a definition', () => {
		const draft = emptyDraft();
		draft.package = ' demo@1.2.3 ';
		draft.args = '--flag\n\n  \nvalue with spaces';
		draft.env = [{ ...emptyEnvSpec(), name: ' TOKEN ' }];

		expect(definitionOf(draft)).toMatchObject({
			package: 'demo@1.2.3',
			bin: null,
			bin_args: ['--flag', 'value with spaces'],
			http: null
		});
		expect(definitionOf(draft).env[0]).toMatchObject({ name: 'TOKEN', secret: true });
		expect(definitionOf(draft).env[0]).not.toHaveProperty('enumText');
	});

	it('sends no bin for a binary, and http settings only in http mode', () => {
		const draft = emptyDraft();
		draft.runtime = 'binary';
		draft.bin = 'left over';
		draft.mode = 'http';
		draft.headers = 'Authorization: Bearer {TOKEN}\nX-Url: https://a.example:8080\n\n';

		const definition = definitionOf(draft);
		expect(definition.bin).toBeNull();
		expect(definition.http).toEqual({
			port_env: 'PORT',
			path: '/mcp',
			headers: { Authorization: 'Bearer {TOKEN}', 'X-Url': 'https://a.example:8080' }
		});
	});

	it('only keeps enum values for an enum', () => {
		const draft = emptyDraft();
		draft.env = [
			{ ...emptyEnvSpec(), name: 'A', type: 'enum', enumText: 'one, two,' },
			{ ...emptyEnvSpec(), name: 'B', enumText: 'stale' }
		];
		const [first, second] = definitionOf(draft).env;
		expect(first?.enum_values).toEqual(['one', 'two']);
		expect(second?.enum_values).toEqual([]);
	});

	it('survives a round trip', () => {
		const definition = {
			mode: 'http' as const,
			runtime: 'node' as const,
			package: 'demo@1.2.3',
			bin: 'server',
			bin_args: ['--a', '--b'],
			env: [
				{
					name: 'A',
					description: '',
					type: 'enum' as const,
					enum_values: ['x', 'y'],
					secret: true,
					required: false,
					conflicts: []
				}
			],
			http: { port_env: 'HTTP_PORT', path: '/rpc', headers: { 'X-A': '{A}' } }
		};
		expect(definitionOf(draftOf(definition))).toEqual(definition);
	});
});

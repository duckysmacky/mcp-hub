import type { CustomServer, EnvSpec, Runtime, ServerMode } from '#lib/api/types.ts';

/** What the form edits: lists and headers are text until they're sent. */
export interface CustomServerDraft {
	mode: ServerMode;
	runtime: Runtime;
	package: string;
	bin: string;
	args: string;
	env: EnvSpecDraft[];
	portEnv: string;
	path: string;
	headers: string;
}

export interface EnvSpecDraft extends EnvSpec {
	enumText: string;
}

const DEFAULT_MCP_PATH = '/mcp';

export function emptyDraft(): CustomServerDraft {
	return {
		mode: 'stdio',
		runtime: 'bun',
		package: '',
		bin: '',
		args: '',
		env: [],
		portEnv: 'PORT',
		path: DEFAULT_MCP_PATH,
		headers: ''
	};
}

export function emptyEnvSpec(): EnvSpecDraft {
	return {
		name: '',
		description: '',
		type: 'string',
		enum_values: [],
		enumText: '',
		// Like the catalog: a forgotten choice must not expose a value.
		secret: true,
		required: false,
		conflicts: []
	};
}

export function draftOf(server: CustomServer): CustomServerDraft {
	return {
		mode: server.mode,
		runtime: server.runtime,
		package: server.package,
		bin: server.bin ?? '',
		args: server.bin_args.join('\n'),
		env: server.env.map((spec) => ({ ...spec, enumText: spec.enum_values.join(', ') })),
		portEnv: server.http?.port_env ?? 'PORT',
		path: server.http?.path ?? DEFAULT_MCP_PATH,
		headers: Object.entries(server.http?.headers ?? {})
			.map(([name, template]) => `${name}: ${template}`)
			.join('\n')
	};
}

function parseHeaders(text: string): Record<string, string> {
	const headers: Record<string, string> = {};
	for (const line of text.split('\n')) {
		const separator = line.indexOf(':');
		// A line without a name still goes through, for the backend to refuse by name.
		const name = (separator < 0 ? line : line.slice(0, separator)).trim();
		if (name) {
			headers[name] = separator < 0 ? '' : line.slice(separator + 1).trim();
		}
	}
	return headers;
}

export function definitionOf(draft: CustomServerDraft): CustomServer {
	return {
		mode: draft.mode,
		runtime: draft.runtime,
		package: draft.package.trim(),
		bin: draft.runtime === 'binary' ? null : draft.bin.trim() || null,
		bin_args: draft.args.split('\n').filter((arg) => arg.trim()),
		env: draft.env.map(({ enumText, ...spec }) => ({
			...spec,
			name: spec.name.trim(),
			enum_values:
				spec.type === 'enum'
					? enumText
							.split(',')
							.map((value) => value.trim())
							.filter(Boolean)
					: []
		})),
		http:
			draft.mode === 'http'
				? {
						port_env: draft.portEnv.trim(),
						path: draft.path.trim(),
						headers: parseHeaders(draft.headers)
					}
				: null
	};
}

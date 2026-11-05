import { i18n } from '#lib/i18n/index.svelte.ts';

/** One step of a client's setup. `copy` is a command or config to hand over as-is. */
export interface GuideStep {
	text: string;
	copy?: string;
}

export interface ClientGuide {
	name: string;
	steps: GuideStep[];
}

const config = (value: unknown) => JSON.stringify(value, null, 2);

/** How to connect each client to an instance. Every one signs in through the hub's OAuth. */
export function clientGuides(slug: string, url: string): ClientGuide[] {
	const text = i18n.t.guides;
	return [
		{
			name: 'Claude Code',
			steps: [
				{ text: text.addServer, copy: `claude mcp add --transport http ${slug} ${url}` },
				{ text: text.claudeCodeSignIn }
			]
		},
		{
			name: text.desktopName,
			steps: [
				{ text: text.desktopOpen },
				{ text: text.desktopPaste, copy: url },
				{ text: text.desktopConnect },
				{ text: text.desktopReachable }
			]
		},
		{
			name: 'Codex',
			steps: [
				{ text: text.addServer, copy: `codex mcp add ${slug} --url ${url}` },
				{ text: text.signIn, copy: `codex mcp login ${slug}` }
			]
		},
		{
			name: 'OpenCode',
			steps: [
				{
					text: text.addToFile('opencode.json'),
					copy: config({ mcp: { [slug]: { type: 'remote', url } } })
				},
				{ text: text.signIn, copy: `opencode mcp auth ${slug}` }
			]
		},
		{
			name: 'Cursor',
			steps: [
				{
					text: text.addToFile('~/.cursor/mcp.json'),
					copy: config({ mcpServers: { [slug]: { url } } })
				},
				{ text: text.cursorSignIn }
			]
		},
		{
			name: 'VS Code',
			steps: [
				{
					text: text.addToFile('.vscode/mcp.json'),
					copy: config({ servers: { [slug]: { type: 'http', url } } })
				},
				{ text: text.vscodeSignIn }
			]
		}
	];
}

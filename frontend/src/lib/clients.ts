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
	return [
		{
			name: 'Claude Code',
			steps: [
				{ text: 'Add the server:', copy: `claude mcp add --transport http ${slug} ${url}` },
				{ text: 'Run /mcp inside Claude Code, pick the server and sign in.' }
			]
		},
		{
			name: 'Claude Desktop and claude.ai',
			steps: [
				{ text: 'Open Customize > Connectors (Settings > Connectors in some versions).' },
				{ text: 'Choose Add custom connector and paste the connection URL:', copy: url },
				{ text: 'Choose Connect and approve access on this hub.' },
				{
					text: "Claude connects from Anthropic's cloud, so this hub must be reachable over public HTTPS."
				}
			]
		},
		{
			name: 'Codex',
			steps: [
				{ text: 'Add the server:', copy: `codex mcp add ${slug} --url ${url}` },
				{ text: 'Sign in:', copy: `codex mcp login ${slug}` }
			]
		},
		{
			name: 'OpenCode',
			steps: [
				{
					text: 'Add this to opencode.json:',
					copy: config({ mcp: { [slug]: { type: 'remote', url } } })
				},
				{ text: 'Sign in:', copy: `opencode mcp auth ${slug}` }
			]
		},
		{
			name: 'Cursor',
			steps: [
				{
					text: 'Add this to ~/.cursor/mcp.json:',
					copy: config({ mcpServers: { [slug]: { url } } })
				},
				{ text: 'Cursor opens a browser to sign in the first time it connects.' }
			]
		},
		{
			name: 'VS Code',
			steps: [
				{
					text: 'Add this to .vscode/mcp.json:',
					copy: config({ servers: { [slug]: { type: 'http', url } } })
				},
				{ text: 'Start the server from that file, and VS Code asks you to sign in.' }
			]
		}
	];
}

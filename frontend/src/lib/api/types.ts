// These mirror the backend's DTOs by hand: change them together.

export type Runtime = 'bun' | 'node' | 'uv' | 'pip' | 'binary';
export type ServerMode = 'stdio' | 'http';
export type SignupMode = 'enabled' | 'disabled' | 'invite-only';

export interface User {
	id: number;
	username: string;
}

export interface AuthConfig {
	signup: SignupMode;
}

export interface EnvSpec {
	name: string;
	description: string;
	type: 'string' | 'number' | 'boolean' | 'enum';
	enum_values: string[];
	secret: boolean;
	required: boolean;
	conflicts: string[];
}

export interface CatalogServer {
	id: string;
	mode: ServerMode;
	runtime: Runtime;
	package: string;
	env: EnvSpec[];
}

export interface EnvStatus {
	var: string;
	is_set: boolean;
	updated_at: string;
	in_catalog: boolean;
	/** Only for variables the catalog marks `secret = false`. */
	value?: string;
}

export interface Instance {
	id: number;
	slug: string;
	server_id: string;
	runtime: Runtime | null;
	package: string | null;
	enabled: boolean;
	needs_attention: string | null;
	url: string;
	env: EnvStatus[];
	missing_required: string[];
	disabled_tools: string[];
	created_at: string;
	updated_at: string;
}

export interface InstanceStatus {
	enabled: boolean;
	needs_attention: string | null;
	mode: ServerMode | null;
	runtime: Runtime | null;
	live_processes: number;
	last_exit: string | null;
	output_tail: string;
}

export interface ToolStatus {
	name: string;
	description: string | null;
	disabled: boolean;
}

export interface Grant {
	id: number;
	instance_id: number;
	slug: string;
	server_id: string;
	client_name_self_reported: string | null;
	client_uri_self_reported: string | null;
	logo_uri_self_reported: string | null;
	redirect_hosts: string[];
	created_at: string;
	last_used_at: string | null;
}

export interface PendingAuthorization {
	client_name_self_reported: string | null;
	redirect_host: string | null;
	redirect_is_loopback: boolean;
	scope: string;
	requested_slug: string | null;
	instance: { slug: string; server_id: string } | null;
}

export interface ConsentDecision {
	redirect_url: string;
}

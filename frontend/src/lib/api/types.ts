// These mirror the backend's DTOs by hand: change them together.

export type Runtime = 'bun' | 'node';
export type ServerMode = 'stdio' | 'http';

export interface User {
	id: number;
	username: string;
}

export interface AuthConfig {
	signup_enabled: boolean;
}

export interface CatalogServer {
	id: string;
	mode: ServerMode;
	default_runtime: Runtime;
	env_vars: string[];
}

export interface EnvStatus {
	var: string;
	is_set: boolean;
	updated_at: string;
	in_catalog: boolean;
}

export interface Instance {
	id: number;
	slug: string;
	server_id: string;
	runtime: Runtime | null;
	enabled: boolean;
	needs_attention: string | null;
	url: string;
	env: EnvStatus[];
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

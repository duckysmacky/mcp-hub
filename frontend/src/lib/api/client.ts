import { goto } from '$app/navigation';
import { resolve } from '$app/paths';
import { error as pageError } from '@sveltejs/kit';

export class ApiError extends Error {
	constructor(
		readonly status: number,
		message: string
	) {
		super(message);
	}
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
	const hasBody = method !== 'GET';
	let response: Response;
	try {
		response = await fetch(`/api${path}`, {
			method,
			credentials: 'same-origin',
			headers: hasBody ? { 'Content-Type': 'application/json' } : undefined,
			body: hasBody ? JSON.stringify(body ?? {}) : undefined
		});
	} catch {
		throw new ApiError(0, 'Could not reach the server. Check your connection and try again.');
	}

	if (!response.ok) {
		// A session that ended mid-visit goes back through login, then returns here.
		if (response.status === 401 && path !== '/auth/login' && path !== '/auth/me') {
			const here = location.pathname + location.search;
			// eslint-disable-next-line svelte/no-navigation-without-resolve -- resolved, plus a query
			await goto(`${resolve('/login')}?next=${encodeURIComponent(here)}`);
		}
		throw new ApiError(response.status, await readError(response));
	}
	if (response.status === 204) {
		return undefined as T;
	}
	return response.json() as Promise<T>;
}

async function readError(response: Response): Promise<string> {
	try {
		const body: { error?: unknown } = await response.json();
		if (typeof body.error === 'string') {
			return body.error;
		}
	} catch {
		// Not JSON, for example a body rejected before reaching the handler.
	}
	return 'The request failed. Try again.';
}

export const api = {
	get: <T>(path: string) => request<T>('GET', path),
	post: <T>(path: string, body?: unknown) => request<T>('POST', path, body),
	put: <T>(path: string, body?: unknown) => request<T>('PUT', path, body),
	patch: <T>(path: string, body?: unknown) => request<T>('PATCH', path, body),
	delete: <T>(path: string) => request<T>('DELETE', path)
};

/** The message to show for anything a page action throws. */
export function messageOf(error: unknown): string {
	if (!(error instanceof ApiError)) {
		return 'Something went wrong. Try again.';
	}
	// The backend writes messages mid-sentence: "slug must be lowercase letters...".
	const message = error.message;
	return message.charAt(0).toUpperCase() + message.slice(1);
}

/** Turns a failed load into SvelteKit's error page, with the readable message. */
export function failPage(error: unknown): never {
	if (error instanceof ApiError) {
		pageError(error.status || 503, error.message);
	}
	throw error;
}

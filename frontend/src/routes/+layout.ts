import { redirect } from '@sveltejs/kit';
import { ApiError, api, failPage } from '#lib/api/client.ts';
import type { User } from '#lib/api/types.ts';
import { safeNextPath } from '#lib/navigation.ts';
import type { LayoutLoad } from './$types';

export const ssr = false;

const LOGGED_OUT_ROUTES = ['/login', '/signup'];

export const load: LayoutLoad = async ({ url }) => {
	const user = await currentUser();
	const isLoggedOutRoute = LOGGED_OUT_ROUTES.includes(url.pathname);

	if (!user && !isLoggedOutRoute) {
		const here = url.pathname + url.search;
		redirect(307, `/login?next=${encodeURIComponent(here)}`);
	}
	if (user && isLoggedOutRoute) {
		redirect(307, safeNextPath(url.searchParams.get('next')));
	}
	return { user };
};

async function currentUser(): Promise<User | null> {
	try {
		return await api.get<User>('/auth/me');
	} catch (error) {
		if (error instanceof ApiError && error.status === 401) {
			return null;
		}
		failPage(error);
	}
}

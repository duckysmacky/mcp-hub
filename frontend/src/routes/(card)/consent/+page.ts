import { ApiError, api, failPage } from '#lib/api/client.ts';
import type { PendingAuthorization } from '#lib/api/types.ts';

export const load = async ({ url }) => {
	const requestId = url.searchParams.get('request') ?? '';
	try {
		const pending = await api.get<PendingAuthorization>(
			`/oauth/pending/${encodeURIComponent(requestId)}`
		);
		return { requestId, pending };
	} catch (error) {
		// Expired, already decided, or never existed: the page explains what to do.
		if (error instanceof ApiError && error.status === 404) {
			return { requestId, pending: null };
		}
		failPage(error);
	}
};

import { api, failPage } from '#lib/api/client.ts';
import type { AuthConfig } from '#lib/api/types.ts';

export const load = async () => ({
	config: await api.get<AuthConfig>('/auth/config').catch(failPage)
});

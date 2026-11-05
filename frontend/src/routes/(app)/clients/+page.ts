import { api, failPage } from '#lib/api/client.ts';
import type { Grant } from '#lib/api/types.ts';

export const load = async () => ({
	grants: await api.get<Grant[]>('/grants').catch(failPage)
});

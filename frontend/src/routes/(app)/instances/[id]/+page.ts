import { api, failPage } from '#lib/api/client.ts';
import type { CatalogServer, Instance, InstanceStatus } from '#lib/api/types.ts';

export const load = async ({ params }) => {
	const [instance, status, servers] = await Promise.all([
		api.get<Instance>(`/instances/${params.id}`),
		api.get<InstanceStatus>(`/instances/${params.id}/status`),
		api.get<CatalogServer[]>('/catalog')
	]).catch(failPage);
	return { instance, status, servers };
};

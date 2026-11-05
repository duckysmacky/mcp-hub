import { api, failPage } from '#lib/api/client.ts';
import type { CatalogServer, Instance, InstanceStatus, ToolStatus } from '#lib/api/types.ts';

export const load = async ({ params }) => {
	const [instance, status, servers, tools] = await Promise.all([
		api.get<Instance>(`/instances/${params.id}`),
		api.get<InstanceStatus>(`/instances/${params.id}/status`),
		api.get<CatalogServer[]>('/catalog'),
		api.get<ToolStatus[]>(`/instances/${params.id}/tools`)
	]).catch(failPage);
	return { instance, status, servers, tools };
};

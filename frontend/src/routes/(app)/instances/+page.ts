import { api, failPage } from '#lib/api/client.ts';
import type { Instance, InstanceStatus } from '#lib/api/types.ts';

export const load = async () => {
	const instances = await api.get<Instance[]>('/instances').catch(failPage);
	// ponytail: one request per instance, fine for a personal list. Add live counts to the list DTO if it grows.
	const statuses = await Promise.all(
		instances.map((instance) => api.get<InstanceStatus>(`/instances/${instance.id}/status`))
	).catch(failPage);
	return {
		rows: instances.map((instance, index) => ({ instance, status: statuses[index] }))
	};
};

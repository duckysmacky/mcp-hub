import { api, failPage } from '#lib/api/client.ts';
import type { CatalogServer, CatalogSettings } from '#lib/api/types.ts';

export const load = async ({ url }) => ({
	servers: await api.get<CatalogServer[]>('/catalog').catch(failPage),
	settings: await api.get<CatalogSettings>('/catalog/settings').catch(failPage),
	requestedSlug: url.searchParams.get('slug')
});

import type { InstanceStatus } from '#lib/api/types.ts';

export interface StatusLabel {
	word: 'RUNNING' | 'IDLE' | 'STOPPED' | 'NEEDS ATTENTION' | 'ERROR';
	tone: 'ok' | 'idle' | 'warn' | 'err';
}

// Exits that aren't failures, as worded by backend/src/process/mod.rs.
const CLEAN_EXITS = ['stopped by the hub', 'exit status: 0'];

/** The last exit, if it's something the user should hear about. */
export function failedExit(status: Pick<InstanceStatus, 'last_exit'>): string | null {
	const { last_exit: exit } = status;
	return exit && !CLEAN_EXITS.includes(exit) ? exit : null;
}

export function statusLabel(status: InstanceStatus, missingRequired: string[]): StatusLabel {
	if (status.needs_attention || missingRequired.length > 0) {
		return { word: 'NEEDS ATTENTION', tone: 'warn' };
	}
	if (!status.enabled) {
		return { word: 'STOPPED', tone: 'idle' };
	}
	if (status.live_processes > 0) {
		return { word: 'RUNNING', tone: 'ok' };
	}
	return failedExit(status) ? { word: 'ERROR', tone: 'err' } : { word: 'IDLE', tone: 'idle' };
}

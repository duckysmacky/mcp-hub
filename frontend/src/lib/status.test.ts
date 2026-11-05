import { expect, test } from 'vitest';
import type { InstanceStatus } from '#lib/api/types.ts';
import { statusLabel } from './status';

const base: InstanceStatus = {
	enabled: true,
	needs_attention: null,
	mode: 'stdio',
	runtime: 'bun',
	live_processes: 0,
	last_exit: null,
	output_tail: ''
};

const wordFor = (change: Partial<InstanceStatus>) => statusLabel({ ...base, ...change }).word;

test('an instance waiting for its first connection is idle', () => {
	expect(wordFor({})).toBe('IDLE');
});

test('live processes mean running, even after an earlier crash', () => {
	expect(wordFor({ live_processes: 2, last_exit: 'exit status: 1' })).toBe('RUNNING');
});

test('only real failures read as errors', () => {
	expect(wordFor({ last_exit: 'exit status: 1' })).toBe('ERROR');
	expect(wordFor({ last_exit: 'signal: 9 (SIGKILL)' })).toBe('ERROR');
	expect(wordFor({ last_exit: 'stopped by the hub' })).toBe('IDLE');
	expect(wordFor({ last_exit: 'exit status: 0' })).toBe('IDLE');
});

test('stopping is a choice, not an error', () => {
	expect(wordFor({ enabled: false, last_exit: 'exit status: 1' })).toBe('STOPPED');
});

test('needing attention wins over everything', () => {
	expect(wordFor({ enabled: false, needs_attention: 'server_removed' })).toBe('NEEDS ATTENTION');
});

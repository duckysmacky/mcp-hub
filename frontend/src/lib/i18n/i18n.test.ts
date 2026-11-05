import { expect, test } from 'vitest';
import { localizeApiError } from './api-errors';
import { en } from './en';
import { plural, ru } from './ru';

test('Russian plurals pick the right form', () => {
	const processes = (count: number) => plural(count, 'процесс', 'процесса', 'процессов');
	expect(processes(1)).toBe('процесс');
	expect(processes(3)).toBe('процесса');
	expect(processes(5)).toBe('процессов');
	expect(processes(11)).toBe('процессов');
	expect(processes(21)).toBe('процесс');
});

test('Russian live process note agrees with its count', () => {
	expect(ru.instance.liveNote(1)).toBe('Завершится 1 активный процесс. ');
	expect(ru.instance.liveNote(2)).toBe('Завершатся 2 активных процесса. ');
	expect(ru.instance.liveNote(5)).toBe('Завершатся 5 активных процессов. ');
});

test('known backend errors translate, keeping their details', () => {
	expect(localizeApiError('the current password is wrong', ru.apiErrors)).toBe(
		'Текущий пароль неверен.'
	);
	expect(localizeApiError('set these values first: A, B', ru.apiErrors)).toBe(
		'Сначала задайте: A, B.'
	);
	expect(localizeApiError("A can't be set together with B; remove that first", ru.apiErrors)).toBe(
		'A нельзя задать вместе с B. Сначала удалите B.'
	);
});

test('unknown errors fall back to the backend text, capitalized', () => {
	expect(localizeApiError('something new', ru.apiErrors)).toBe('Something new');
	expect(localizeApiError('the current password is wrong', en.apiErrors)).toBe(
		'The current password is wrong'
	);
});

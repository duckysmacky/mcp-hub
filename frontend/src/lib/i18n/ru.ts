import type { Messages } from './en.ts';

const pluralRules = new Intl.PluralRules('ru');

/** Picks the Russian plural form: 1 процесс, 2 процесса, 5 процессов. */
export function plural(count: number, one: string, few: string, many: string): string {
	const forms: Partial<Record<Intl.LDMLPluralRule, string>> = { one, few, many };
	return forms[pluralRules.select(count)] ?? many;
}

export const ru: Messages = {
	common: {
		cancel: 'Отмена',
		copy: 'Копировать',
		copied: 'Готово',
		copiedName: (name) => `Скопировано: ${name}`,
		copyName: (name) => `Копировать: ${name}`,
		show: 'Показать',
		hide: 'Скрыть',
		dismiss: 'Закрыть',
		clipboardFailed: 'Нет доступа к буферу обмена. Выделите текст и скопируйте вручную.',
		required: 'Обязательно',
		selfReported: 'Со слов клиента',
		localhost: 'Локально',
		instanceContext: (slug) => `Инстанс ${slug}`
	},
	errors: {
		unreachable: 'Не удалось связаться с сервером. Проверьте соединение и повторите.',
		requestFailed: 'Запрос не выполнен. Повторите.',
		generic: 'Что-то пошло не так. Повторите.',
		wrongCredentials: 'Неверное имя пользователя или пароль.',
		pageMissing: 'Такой страницы нет',
		pageFailed: 'Запрос не выполнен',
		errorLabel: (status) => `Ошибка ${status}`,
		goToInstances: 'К инстансам'
	},
	nav: {
		primary: 'Основная навигация',
		instances: 'Инстансы',
		clients: 'Клиенты',
		settings: 'Настройки',
		signOut: 'Выйти'
	},
	status: {
		RUNNING: 'РАБОТАЕТ',
		IDLE: 'ОЖИДАЕТ',
		STOPPED: 'ОСТАНОВЛЕН',
		'NEEDS ATTENTION': 'НУЖНО ВНИМАНИЕ',
		ERROR: 'ОШИБКА'
	},
	auth: {
		signIn: 'Войти',
		signingIn: 'Вход',
		createAccount: 'Создать аккаунт',
		creatingAccount: 'Создание аккаунта',
		username: 'Имя пользователя',
		password: 'Пароль',
		passwordHelp: 'Не меньше 8 символов.',
		inviteCode: 'Код приглашения',
		inviteHelp: 'Для регистрации на этом хабе нужно приглашение. Попросите код у оператора.',
		noAccount: 'Нет аккаунта?',
		createOne: 'Создать',
		haveInvite: 'Есть приглашение?',
		haveAccount: 'Уже есть аккаунт?',
		signupClosedTag: 'Регистрация закрыта',
		signupClosed: 'Оператор хаба отключил новые аккаунты. Попросите его создать аккаунт для вас.',
		backToSignIn: 'Назад ко входу',
		signInTitle: 'Вход - MCP Hub',
		signupTitle: 'Создание аккаунта - MCP Hub'
	},
	slug: {
		label: 'Слаг инстанса',
		help: 'Часть вашего URL подключения. Строчные латинские буквы, цифры и дефисы.',
		invalid: 'Используйте строчные латинские буквы, цифры и дефисы, в начале буква или цифра.'
	},
	instances: {
		title: 'Инстансы',
		crumb: 'инстансы',
		newInstance: 'Новый инстанс',
		emptyMark: '[ 0 инстансов ]',
		emptyTitle: 'Серверов пока нет',
		emptyBody:
			'Выберите сервер из каталога, задайте слаг и добавьте свои учётные данные. Вы получите личный URL, который можно вставить в Claude.'
	},
	newInstance: {
		crumb: 'новый',
		server: 'Сервер',
		create: 'Создать инстанс',
		creating: 'Создание инстанса',
		cancel: 'Отмена',
		emptyMark: '[ 0 серверов ]',
		emptyTitle: 'Каталог пуст',
		emptyBody:
			'Здесь запускаются только серверы из каталога оператора, а на этом хабе ни один не включён. Добавьте сервер в catalog.toml и перезапустите хаб.',
		variables: 'Переменные',
		noVariables: 'Нет'
	},
	custom: {
		option: 'свой сервер (задать самому)',
		warningTag: 'Запуск на хабе',
		warningLead: 'Всё, что вы здесь зададите, запускается на самом хабе без песочницы.',
		warningBody:
			'Устанавливайте только то, чему доверяете. Описание хранится незашифрованным: учётные данные указывайте в секретных переменных, а не в аргументах или заголовках.',
		definition: 'Описание сервера',
		runtime: 'Среда выполнения',
		mode: 'Режим',
		package: 'Пакет',
		packageHelp:
			'имя@точная-версия. Устанавливается при сохранении, установочные скрипты отключены.',
		binaryPath: 'Бинарный файл',
		binaryPathHelp: 'Абсолютный путь на хабе или имя для поиска в его PATH.',
		bin: 'Точка входа',
		binHelp: 'Нужна, только если в пакете их несколько.',
		binPython: 'Скрипт, который устанавливает пакет. Для Python обязателен.',
		args: 'Аргументы',
		argsHelp: 'По одному в строке, передаются как есть. Оболочка не используется.',
		portEnv: 'Переменная порта',
		path: 'Путь эндпоинта',
		headers: 'Заголовки запросов',
		headersHelp:
			'По одному в строке: Имя: значение. {VAR} заменяется этой переменной, которая не попадает в процесс.',
		variables: 'Переменные окружения',
		variablesHelp: 'Процесс получит только их. Значения задаются после создания инстанса.',
		newVariable: 'Новая переменная',
		varName: 'Имя',
		varType: 'Тип',
		varEnum: 'Допустимые значения через запятую',
		varDescription: 'Описание',
		varSecret: 'Секрет',
		varRequired: 'Обязательная',
		addVariable: 'Добавить переменную',
		removeVariable: 'Удалить',
		save: 'Сохранить описание',
		saveTitle: 'Сохранить это описание?',
		saveDetail: 'Запущенные процессы остановятся, а изменившийся пакет установится заново.',
		saved: 'Описание сохранено'
	},
	instance: {
		label: 'Инстанс',
		metaCatalog: (id) => `каталог: ${id}`,
		metaCustom: 'свой сервер',
		metaMode: (mode) => `режим: ${mode}`,
		metaRuntime: (runtime) => `среда: ${runtime}`,
		start: 'Запустить',
		starting: 'Запуск',
		restart: 'Перезапустить',
		restarting: 'Перезапуск',
		stop: 'Остановить',
		stopping: 'Остановка',
		needsAttention: 'Нужно внимание',
		serverRemovedLead: 'Этот сервер удалён из каталога.',
		serverRemovedBody: 'Он не запустится, пока оператор не вернёт его. Инстанс можно удалить.',
		serverDisabledLead: 'Оператор отключил этот сервер.',
		serverDisabledBody: 'Он не запустится, пока его снова не включат. Инстанс можно удалить.',
		customDisabledLead: 'Оператор отключил пользовательские серверы.',
		customDisabledBody:
			'Этот сервер не запустится, пока их снова не включат. Инстанс можно удалить.',
		secretsResetLead: 'Ваши сохранённые секреты удалены.',
		secretsResetBody:
			'Оператор сбросил ваш пароль, а это стирает секреты. Задайте их заново и запустите инстанс.',
		missingLead: 'Не заданы обязательные значения.',
		missingBody: (names) => `Перед запуском задайте: ${names}.`,
		connection: 'Подключение',
		connectionUrl: 'URL подключения',
		settings: 'Настройки',
		saveSettings: 'Сохранить настройки',
		settingsSaved: 'Настройки сохранены',
		renameTitle: (slug) => `Переименовать в ${slug}?`,
		renameDetail:
			'URL подключения изменится, поэтому все подключённые приложения придётся добавить заново. Запущенные процессы остановятся.',
		rename: 'Переименовать инстанс',
		deleteHeading: 'Удаление инстанса',
		deleteBody: 'Удаляет сохранённые значения и URL подключения. Это нельзя отменить.',
		deleting: 'Удаление',
		deleteTitle: 'Удалить инстанс?',
		deleteDetail: 'Сохранённые значения будут удалены, а все приложения отключены.',
		deleted: (slug) => `${slug} удалён`,
		stopTitle: 'Остановить инстанс?',
		stopDetail: 'Подключённые приложения потеряют доступ, пока вы его снова не запустите.',
		stopAction: 'Остановить инстанс',
		restartTitle: 'Перезапустить инстанс?',
		restartDetail:
			'Подключённые приложения переподключатся сами, а незавершённая работа отменится.',
		restartAction: 'Перезапустить инстанс',
		restarted: (slug) => `${slug} перезапущен`,
		liveNote: (count) =>
			`${plural(count, 'Завершится', 'Завершатся', 'Завершатся')} ${count} ${plural(count, 'активный процесс', 'активных процесса', 'активных процессов')}. `,
		statusPanel: 'Статус',
		live: 'онлайн',
		kvStatus: 'Статус',
		kvMode: 'Режим',
		kvRuntime: 'Среда',
		kvPackage: 'Пакет',
		kvProcesses: 'Процессы',
		liveCount: (count) => `активно: ${count}`,
		kvLastExit: 'Выход'
	},
	env: {
		heading: 'Переменные окружения',
		hint: 'секреты только для записи',
		required: 'Обязательные',
		optional: 'Необязательные',
		optionalCount: (set, total) => `задано ${set} из ${total}`,
		secretTag: 'Секрет',
		secretPlaceholder: 'Больше не будет показан',
		notSetOption: 'Не задано',
		replace: 'Заменить',
		remove: 'Удалить',
		stateSet: 'ЗАДАНО',
		stateNotSet: 'НЕ ЗАДАНО',
		stateUnused: 'НЕ ИСПОЛЬЗУЕТСЯ',
		setOn: (date) => `задано ${date}`,
		conflictsWith: (names) => `Конфликтует с ${names}. Сначала удалите это значение.`,
		alternativeTo: (names) => `Альтернатива ${names}: задайте что-то одно.`,
		save: 'Сохранить',
		saving: 'Сохранение',
		saved: (name) => `${name} сохранено`,
		savedRestart: (name) => `${name} сохранено. Перезапустите, чтобы применить.`,
		removed: (name) => `${name} удалено`,
		removeTitle: (name) => `Удалить ${name}?`,
		removeDetail: 'Значение будет удалено. Запущенные процессы хранят свою копию до перезапуска.',
		removeAction: 'Удалить переменную',
		restart: 'Перезапустить'
	},
	tools: {
		heading: 'Фильтр инструментов',
		enabledCount: (enabled, total) => `включено ${enabled} из ${total}`,
		empty:
			'Инструментов пока не видно. Они появятся, когда подключённое приложение запросит список.',
		help: 'Выключенные инструменты скрыты от приложений, а их вызовы отклоняются. Изменения применяются сразу.',
		search: 'Поиск инструментов',
		noMatch: 'Ничего не найдено.',
		on: 'ВКЛ',
		off: 'ВЫКЛ'
	},
	clients: {
		title: 'Подключённые клиенты',
		pageTitle: 'Клиенты - MCP Hub',
		emptyMark: '[ 0 клиентов ]',
		emptyTitle: 'Нет подключённых приложений',
		emptyBody:
			'Добавьте URL подключения инстанса в Claude или другое приложение. Оно появится здесь, когда вы одобрите доступ.',
		unnamed: 'Приложение без имени',
		client: 'Клиент',
		redirectHost: 'Хост перенаправления',
		instance: 'Инстанс',
		created: 'Создан',
		lastUsed: 'Использован',
		actions: 'Действия',
		never: 'Никогда',
		revoke: 'Отозвать',
		revoking: 'Отзыв',
		revokeTitle: (name) => `Отозвать доступ: ${name}?`,
		revokeDetail: (slug) =>
			`Клиент потеряет доступ к ${slug}. Запущенные процессы инстанса остановятся, а другие подключённые приложения переподключатся сами.`,
		revoked: (name) => `Доступ отозван: ${name}`
	},
	settings: {
		title: 'Настройки',
		pageTitle: 'Настройки - MCP Hub',
		account: 'Аккаунт',
		username: 'Имя',
		password: 'Смена пароля',
		currentPassword: 'Текущий пароль',
		newPassword: 'Новый пароль',
		newPasswordHelp: 'Не меньше 8 символов. В других браузерах сессии завершатся.',
		savePassword: 'Сменить пароль',
		savingPassword: 'Смена пароля',
		passwordChanged: 'Пароль изменён',
		appearance: 'Оформление',
		theme: 'Тема',
		themeSystem: 'Системная',
		themeDark: 'Тёмная',
		themeLight: 'Светлая',
		themeHelp: 'Системная следует настройкам устройства. Выбор хранится в этом браузере.',
		language: 'Язык',
		languageHelp:
			'Хранится в этом браузере. Описания переменных и вывод серверов остаются без перевода.'
	},
	consent: {
		label: 'Разрешение доступа',
		pageTitle: 'Разрешение доступа - MCP Hub',
		failed: 'Сбой',
		expiredTitle: 'Срок запроса истёк',
		expiredBody: 'Начните подключение из приложения заново.',
		noInstanceTitle: (slug) => `У вас нет инстанса ${slug}`,
		noInstanceBody: 'Приложение запросило этот инстанс. Создайте его и проверьте снова.',
		createInstance: 'Создать инстанс',
		checkAgain: 'Проверить снова',
		client: 'Клиент',
		instance: 'Инстанс',
		unnamed: 'Приложение без имени',
		title: (client, slug) => `Разрешить ${client} использовать ваш инстанс ${slug}?`,
		loopbackLabel: '[ ! ] Перенаправление на ваш компьютер',
		loopbackHelp:
			'Доступ получит приложение на вашем компьютере. Разрешайте, только если вы сами только что начали этот вход с этого устройства.',
		redirectLabel: 'Вы вернётесь на',
		redirectHelp: 'Имя клиента указано им самим. Доверяйте хосту перенаправления, а не имени.',
		scope:
			'Оно сможет от вашего имени вызывать все включённые инструменты этого инстанса, пока вы не отзовёте доступ в разделе «Клиенты».',
		deny: 'Отклонить',
		denying: 'Отклонение',
		approve: 'Разрешить',
		approving: 'Разрешение',
		signedInAs: (username) => `Вы вошли как ${username}`
	},
	log: {
		heading: 'Вывод',
		hint: 'со скрытыми секретами',
		aria: 'Вывод сервера',
		empty: 'Вывода пока нет. Он появится, когда подключится приложение.'
	},
	guides: {
		summary: 'Подключить приложение',
		step: (client, step) => `${client}, шаг ${step}`,
		addServer: 'Добавьте сервер:',
		signIn: 'Войдите:',
		claudeCodeSignIn: 'Выполните /mcp в Claude Code, выберите сервер и войдите.',
		desktopName: 'Claude Desktop и claude.ai',
		desktopOpen: 'Откройте Customize > Connectors (в некоторых версиях Settings > Connectors).',
		desktopPaste: 'Выберите Add custom connector и вставьте URL подключения:',
		desktopConnect: 'Нажмите Connect и разрешите доступ на этом хабе.',
		desktopReachable:
			'Claude подключается из облака Anthropic, поэтому хаб должен быть доступен по публичному HTTPS.',
		addToFile: (file) => `Добавьте в ${file}:`,
		cursorSignIn: 'При первом подключении Cursor откроет браузер для входа.',
		vscodeSignIn: 'Запустите сервер из этого файла, и VS Code предложит войти.'
	},
	apiErrors: [
		[/^invalid username or password$/, () => 'Неверное имя пользователя или пароль.'],
		[/^not logged in$/, () => 'Вы не вошли в аккаунт.'],
		[/^not found$/, () => 'Не найдено.'],
		[/^internal error$/, () => 'Внутренняя ошибка сервера.'],
		[/^too many attempts; try again later$/, () => 'Слишком много попыток. Повторите позже.'],
		[/^sign-up is disabled$/, () => 'Регистрация отключена.'],
		[/invite/i, () => 'Этот код приглашения недействителен.'],
		[/^username is taken$/, () => 'Это имя пользователя занято.'],
		[
			/^username must be 1-(\d+) letters, digits, or - _ \.$/,
			(match) => `Имя пользователя: от 1 до ${match[1]} латинских букв, цифр или символов - _ .`
		],
		[
			/^password must be at least (\d+) characters$/,
			(match) => `Пароль должен быть не короче ${match[1]} символов.`
		],
		[/^password is too long$/, () => 'Пароль слишком длинный.'],
		[/^the current password is wrong$/, () => 'Текущий пароль неверен.'],
		[/^you have no instance with this slug$/, () => 'У вас нет инстанса с таким слагом.'],
		[
			/^you already have an instance with this slug$/i,
			() => 'У вас уже есть инстанс с таким слагом.'
		],
		[/^this server was removed from the catalog$/, () => 'Этот сервер удалён из каталога.'],
		[/^the operator has turned this server off$/, () => 'Оператор отключил этот сервер.'],
		[
			/^slug must be lowercase letters/,
			() => 'Слаг: строчные латинские буквы, цифры и дефисы, в начале буква или цифра.'
		],
		[/^unknown server$/, () => 'Неизвестный сервер.'],
		[
			/^the operator has turned custom servers off$/,
			() => 'Оператор отключил пользовательские серверы.'
		],
		[
			/^only a custom instance has a definition to change$/,
			() => 'Описание можно менять только у своего сервера.'
		],
		[
			/^you can have at most (\d+) instances$/,
			(match) =>
				`Можно создать не больше ${match[1]} ${plural(Number(match[1]), 'инстанса', 'инстансов', 'инстансов')}.`
		],
		[
			/^this server doesn't allow that variable$/,
			() => 'Этот сервер не допускает такую переменную.'
		],
		[/^value is too long$/, () => 'Значение слишком длинное.'],
		[/^value contains a NUL byte$/, () => 'Значение содержит байт NUL.'],
		[
			/^(\S+) can't be set together with (\S+); remove that first$/,
			(match) => `${match[1]} нельзя задать вместе с ${match[2]}. Сначала удалите ${match[2]}.`
		],
		[/^set these values first: (.+)$/, (match) => `Сначала задайте: ${match[1]}.`],
		[/^(\S+) is required and can't be empty$/, (match) => `${match[1]} не может быть пустым.`],
		[/^(\S+) must be a number$/, (match) => `${match[1]} должно быть числом.`],
		[/^(\S+) must be true or false$/, (match) => `${match[1]} должно быть true или false.`],
		[/^(\S+) must be one of: (.+)$/, (match) => `${match[1]} должно быть одним из: ${match[2]}.`],
		[/^URLs must use https$/, () => 'URL должен использовать https.'],
		[/^the URL has no host$/, () => 'В URL нет хоста.'],
		[/^the URL's host could not be resolved$/, () => 'Не удалось определить адрес хоста из URL.'],
		[/^the URL points at an internal address$/, () => 'URL указывает на внутренний адрес.'],
		[
			/^this host is only allowed on ports 80 and 443$/,
			() => 'Этот хост разрешён только на портах 80 и 443.'
		]
	]
};

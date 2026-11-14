`# Changelog

[English](CHANGELOG.md) · **Русский**

Здесь фиксируются все заметные изменения MMA. Формат — [Keep a Changelog](https://keepachangelog.com/ru/1.1.0/). Версии — по [Semantic Versioning](https://semver.org/lang/ru/); до `1.0.0` несовместимое изменение повышает минорную версию.

В каждой записи влияние на три поверхности совместимости указано отдельно (см. [README §9](README.ru.md#9-совместимость-и-версионирование)):

| Метка | Поверхность | Что это |
|---|---|---|
| **Wire** | раскладка кадра и `MMA_VERSION` | байты в соединении |
| **Rust** | crate `MMA` (`rlib`) | публичные элементы `src/` |
| **ABI** | `libMMA` + `include/mma.h` | экспортируемые символы, раскладки структур, контракты callback-ов |


### Добавлено


- **Непрозрачный handle запроса и явное управление временем жизни.** `MMA_Request` стал opaque handle; добавлены `mma_request_retain` / `mma_request_release`, позволяющие сохранить запрос после возврата callback. Добавлены `mma_request_get_info`, индексный доступ к options (`mma_request_option_count`, `mma_request_option_at`) и `mma_request_is_cancelled`. *ABI*
- **Явное асинхронное завершение route.** Теперь route-callback получает `MMA_Request *` и возвращает `void`; C создаёт response через `mma_response_new`, setters и `mma_request_complete`. Завершить запрос можно после возврата callback, предварительно удержав его через `retain`. Добавлены коды `MMA_REQUEST_ALREADY_COMPLETED` и `MMA_REQUEST_CANCELLED`. *ABI*
- **Непрозрачный response.** `MMA_Response` теперь является opaque-объектом, памятью которого управляет библиотека; setters payload/options копируют входные буферы. Для `mma_request_complete` документировано владение response при успехе, повторном завершении и отмене. *ABI*
- **Обновлённые C-примеры.** Оба примера используют opaque handle, завершение route, channel/lifecycle notifications, `MMA_ServerHandle` и корректное завершение сервера. TLS-пример задаёт `tls_handshake_timeout` в миллисекундах. *ABI / examples*
- Добавлен `.gitattributes` с нормализацией текстовых файлов к LF, чтобы диффы оставались стабильными между Windows и Unix. *Репозиторий / tooling*

### Изменено

- **C ABI — несовместимая переработка.** `MMA_Request` и `MMA_Response` больше не являются публичными структурами, передаваемыми по значению. Удалены/заменены старые `MMA_Option`, `MMA_ChannelHandlerCallback`, callback с возвратом `MMA_Response`, alias `mma_server_route` и имя `MMA_RESPONSE_MESSAGE`. Новые callback-и возвращают `void`; channel/lifecycle callback-и получают `MMA_Request *`, `connection_id` и изменяемый `user_data`. Все C/C++ приложения необходимо пересобрать с актуальным `include/mma.h`. *ABI*
- **Модель выполнения C-callback.** Callback вызывается синхронно из Tokio runtime task; он не переносится в `spawn_blocking`. Блокирующий C-callback занимает worker Tokio и не может быть принудительно прерван через `request_timeout`. Для блокирующего I/O и CPU-задач приложение должно удержать запрос и передать работу в собственный ограниченный executor. *ABI / поведение*
- **Channel-callback-и являются уведомлениями.** Они не формируют route-response. `mma_request_complete` для channel/lifecycle запроса возвращает `MMA_INVALID_ARGUMENT`; владение response остаётся у вызывающей стороны. *ABI*
- **Владение запросом и response.** Каждый успешный `retain` должен иметь ровно один `release`. Указатели из request info и option API заимствованы и действительны только пока существует ссылка на запрос. Для корректного route-запроса completion забирает response при `MMA_OK`, `MMA_REQUEST_ALREADY_COMPLETED` и `MMA_REQUEST_CANCELLED`; при `MMA_INVALID_ARGUMENT` response остаётся у вызывающей стороны. *ABI*
- **Options.** Повторяющиеся ключи теперь разрешены encoder и decoder. Пустые key/value, усечённые записи, несовпадение числа options и лишние байты по-прежнему считаются ошибкой. Если приложению нужна уникальность ключей, оно должно проверять её самостоятельно. *Wire (проверка decoder)*
- **Framer encoder.** Теперь резервирует оценённую ёмкость кадра и явно увеличивает логическую длину буфера до размера header перед записью его полей. Framer также начинает работу с кешированной копией текущей конфигурации. *Внутренние изменения / производительность и корректность*
- **Workflow.** CI компилирует C-примеры на Linux и Windows/MSVC, а также проверяет C++-совместимость заголовка, используя opaque pointers вместо попытки создавать объекты непрозрачных типов. Пакеты native release по-прежнему собираются для Windows x86-64, Linux x86-64, macOS x86-64 и macOS arm64. *Только CI/release*

### Исправлено

- В TLS-примере `tls_handshake_timeout` исправлен на миллисекунды (`10000` = 10 секунд).
- C-примеры игнорируют `SIGPIPE` на POSIX до начала обслуживания соединений: закрытие peer во время записи больше не завершает процесс примера.
- Удалён неиспользуемый импорт `AHashSet` из Framer.
- Исправлен комментарий в реализации C API, который ранее неверно описывал поток выполнения callback: вызов выполняется синхронно на worker Tokio.

### Миграция

- Пересоберите каждый C/C++-потребитель с новым `include/mma.h` и свяжите его с соответствующей версией `libMMA`. Не смешивайте заголовок из предыдущего snapshot с текущей динамической библиотекой.
- Вместо прямого доступа к полям request используйте `mma_request_get_info` и `mma_request_option_count` / `mma_request_option_at`.
- Замените route-callback-и, возвращавшие `MMA_Response`, на `void` callback-и с вызовом `mma_request_complete`.
- Перед передачей request в другой поток или executor вызовите `mma_request_retain`; каждый успешный `retain` должен иметь ровно один `release`.
- `mma_server_get_handle` можно вызывать только после успешного `mma_server_start`. Освобождайте handle до уничтожения сервера.


## [0.1.4] - 0.1.5

### Изменено

- **Модель доставки в каналы — Rust, ABI (поведение).** Общий `tokio::sync::broadcast` (1024 сообщения, фильтрация по `conn_id` в каждом соединении) заменён на `ChannelHub`: таблицу из одной очереди `mpsc` на каждое channel-соединение, по **256 сообщений**.
  - Сообщение для одного `conn_id` идёт только в это соединение (поиск по хеш-таблице), а не будит все channel-соединения.
  - Сообщения больше не теряются через `Lagged`. При полной очереди отправитель ждёт (`send_to_channel`) или получает ошибку (`try_send_to_channel`, C ABI).
  - Неизвестный `conn_id` теперь ошибка (`"No such channel"`). Рассылка при отсутствии channel-соединений теперь `Ok` (раньше `Err`).
  - **Компромисс:** `send_to_channel(0, ..)` ждёт каждое соединение по очереди. Один клиент, который не читает, останавливает рассылку для всех остальных. Замер на loopback с `examples/main_server`: один молчащий клиент, сообщения по 64 КиБ — второй клиент получил 1322 из 3000 и дальше ничего, включая более позднее маленькое сообщение.
- **Rust API (breaking).**
  - `ServerHandle::send_to_channel` теперь `async fn(&self, id: u32, item: Response) -> Result<(), String>` (раньше синхронная, `Result<(), ()>`). Вызывающий код должен делать `.await`.
  - Новый `ServerHandle::try_send_to_channel(&self, id, item) -> Result<(), String>` без ожидания. При `id = 0` пробует все каналы и при частичном сбое возвращает `Partial broadcast: N/M channels accepted the message`.
  - Удалён `core::commands::ChannelSendedMessage`.
  - Новый публичный `core::server::ChannelHub` (`register`, `disconnect`, `send`, `try_send`).
- **C ABI (поведение, сигнатуры не менялись).** `mma_server_handle_send_to_channel` теперь использует путь без ожидания. Возвращает `MMA_SERVER_ERROR` для неизвестного `conn_id`, полной очереди (256) или частично принятой рассылки. Код, считавший `MMA_OK` признаком «подписчик существует», нужно пересмотреть.
- **Лимит запросов на соединение убран.** Семафор на 256 разрешений на соединение стал семафором на одно разрешение, которое упорядочивает решения о состоянии соединения. Единственный предел запросов в полёте — `max_in_flight` на весь сервер, так что одно соединение может занять его целиком.
- `ChannelConnect` с неверным route: payload `BadRequest` теперь `"Route must match the opcode"` (было `"Route must be doesn't match by opcode"`). *Wire (только текст)*
- `examples/main_server.rs` делает `await` у `send_to_channel`.

### Исправлено

- **`ChannelConnect` больше не создаёт канал до проверки route** (известная проблема 1 из 0.1.3). Неверный route ничего не оставляет.
- **`CHANNEL_DISCONNECT` больше не получает ответ `NotFound`** (известная проблема 2 из 0.1.3). После хука канал снимается с регистрации, соединение закрывается без ответа; кадр в channel-обработчики не попадает.
- **Закрытое соединение снимает свой канал с регистрации** при EOF, ошибке чтения, остановке сервера и после `CHANNEL_DISCONNECT`. (Пользовательский disconnect-хук при обрыве соединения по-прежнему не вызывается; см. ниже.)
- **Гонка состояния на новом соединении.** Решение о состоянии для каждого кадра принимается под семафором на одно разрешение, поэтому два кадра больше не могут оба пройти как `Fresh` (например, `Once` и `ChannelConnect`, или два `ChannelConnect`). Найдено чтением кода; автотеста на гонку нет.
- **Panic в хуках локализуется.** Panic в connect-хуке откатывает регистрацию канала и даёт `InternalError`; panic в disconnect-хуке даёт `InternalError`.
- Внутренняя ошибка регистрации при connect даёт `InternalError` вместо наполовину открытого канала.
- Убран отладочный `println!("connect, N")` при каждом подключении к каналу.


## [0.1.3] - 2026-11-02

Тег `v0.1.3` (коммит `95a5abb`). Изменения с коммита `432905c` («added README»). В `Cargo.toml` по-прежнему `0.1.0`, хотя теги доходят до `v0.1.3`; `MMA_VERSION` по-прежнему `1`.

### Добавлено

- **Хуки жизненного цикла соединения** — пользовательский код при открытии канала и при его закрытии клиентом. *Rust, ABI*
  - Rust: `Router::register_on_channel_connect_handler(f)` и `Router::register_on_channel_disconnect_handler(f)`, где `f: Fn(Request, u32) -> impl Future<Output = ()>`. Повторный вызов заменяет хук. Методы вызова `Router::on_channel_connect` / `on_channel_disconnect` публичные.
  - C: `mma_server_register_on_channel_connect_handler` и `mma_server_register_on_channel_disconnect_handler` (сигнатура как у `mma_server_register_channel_handler`, без route). Добавлены в `mma.h` и экспортируются `libMMA`; эти два символа — единственные добавления ABI.
- **Зарезервированные route** `CHANNEL_CONNECT` и `CHANNEL_DISCONNECT`. `router::ReservedRoutes` стал `pub`, получил вариант `CHANNEL_DISCONNECT` и реализует `Display`. *Rust, Wire*
- `Request` реализует `Clone`. *Rust*
- `examples/main_server.rs` показывает оба хука (они рассылают во все каналы сообщения `Connect: …` / `Disconnect: …`).
- `mma_gui_client.py`: тестовый клиент на Tkinter, только стандартная библиотека Python. Принимает полную раскладку (`FramerConfig`), отправляет все три opcode запроса, декодирует ответы, есть самотест кодека.
- README (`README.md`, `README.ru.md`): §5.5 (зарезервированные route и хуки), примеры хуков для Rust и C, новые C-функции, обновлённые правила потоков и структура репозитория. `CHANGELOG.md` и `CHANGELOG.ru.md`.
- Workflow GitHub Actions `.github/workflows/native-release.yml`: на каждый push запускает `cargo test`, собирает release-библиотеки, компилирует C-пример с `-Werror` и проверяет, что `mma.h` компилируется как C++. На тегах `v*` собирает нативные пакеты для Windows x86-64, Linux x86-64, macOS x86-64 и macOS arm64 (библиотека + `mma.h`, на Windows ещё импортная библиотека MSVC, с файлами SHA-256) и публикует GitHub Release. Первая версия workflow компилировала `native-bridge/bridge.c`, которого нет в `master`; последующие коммиты этот шаг убрали.

### Изменено

- **Wire — семантика запросов (не раскладка кадра):**
  - Запрос `ChannelConnect` теперь обязан нести route `CHANNEL_CONNECT` (без учёта регистра). Любой другой route даёт `BadRequest` с payload `"Route must be doesn't match by opcode"`, соединение остаётся `Fresh`. Клиенты, отправлявшие произвольный route, нужно обновить.
  - `ChannelMessage` с route `CHANNEL_DISCONNECT` в состоянии `Channel` запускает disconnect-хук и затем закрывает соединение (после завершения активных задач). В состоянии `Fresh` — `BadRequest` («Channel not connected»).
  - `MMA_VERSION` **не** повышена. Решение за мейнтейнером: поднять до `2` либо принять изменение внутри `1` и описать его (сделано в README §5.5 и §9).
- **ABI — модель выполнения callback-ов (поведение, не сигнатуры):** C-callbacks (route, channel-обработчики, хуки) теперь выполняются в blocking-пуле Tokio, а не на асинхронных worker-потоках. Одновременно работает не более `min(max_in_flight, 128)`; остальные ждут слот, и ожидание входит в `request_timeout`. Callback, переживший `request_timeout`, продолжает держать слот, а `mma_server_stop` дожидается работающих callback-ов, поэтому может превысить `shutdown_grace`. Callbacks, рассчитывавшие на конкретный поток или чувствительные к прежней степени параллелизма, нужно перепроверить. *ABI*
- **ABI — аргументы callback-ов:** представление `MMA_Request` для channel-callbacks и хуков строится из самого запроса (с реальной строкой route); для обычных route содержимое прежнее. Значение, отличное от `MMA_OK`, из channel-callback или хука пишется в stderr; клиенту оно по-прежнему не передаётся. *ABI*
- **ABI — проверки аргументов:** `valid_slice` теперь отклоняет и длины больше `isize::MAX`; ошибка инициализации router печатается в stderr до того, как `mma_server_start` вернёт `MMA_SERVER_ERROR`. *ABI*
- Внутренние (приватные) изменения в `src/network/network.rs`: у цикла соединения появился токен отмены `disconnect`, `handle_cmd` принимает дополнительное замыкание disconnect, а замыкание connect возвращает идентификатор канала. Публичные элементы не менялись. Route-callback теперь передаёт response в blocking-задачу, что заодно убирает утечку response при отбрасывании future запроса до запуска (по чтению кода; не измерено).
- Форматирование и комментарии `// SAFETY:` в `src/c_api.rs`; убран неиспользуемый импорт `Ipv4Addr`.

### Исправлено

- Синхронный C-callback больше не блокирует асинхронный worker Tokio (см. *Изменено → ABI*). Это снимает главный риск голодания, названный в прежнем README («callbacks выполняются на worker-потоках»).


[Unreleased]: https://github.com/Edestuss0/MMA-Network-protocol/compare/v0.1.4...HEAD
[0.1.4]: https://github.com/Edestuss0/MMA-Network-protocol/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/Edestuss0/MMA-Network-protocol/compare/432905c...v0.1.3
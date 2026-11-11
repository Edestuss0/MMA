`# Changelog

[English](CHANGELOG.md) · **Русский**

Здесь фиксируются все заметные изменения MMA. Формат — [Keep a Changelog](https://keepachangelog.com/ru/1.1.0/). Версии — по [Semantic Versioning](https://semver.org/lang/ru/); до `1.0.0` несовместимое изменение повышает минорную версию.

В каждой записи влияние на три поверхности совместимости указано отдельно (см. [README §9](README.ru.md#9-совместимость-и-версионирование)):

| Метка | Поверхность | Что это |
|---|---|---|
| **Wire** | раскладка кадра и `MMA_VERSION` | байты в соединении |
| **Rust** | crate `MMA` (`rlib`) | публичные элементы `src/` |
| **ABI** | `libMMA` + `include/mma.h` | экспортируемые символы, раскладки структур, контракты callback-ов |

## [Unreleased]

Изменения после `v0.1.4` (коммит `b6b8579`). Предлагаемая версия: **0.2.0** — изменились пути модулей и `Config` (Rust) и размер `MMA_ServerConfig` (C ABI). `MMA_VERSION` остаётся `1`: раскладка кадра и семантика запросов не менялись; TLS оборачивает неизменный поток байтов.

### Добавлено

- **TLS (серверная сторона)** на `tokio-rustls` 0.26 / rustls, без клиентских сертификатов. *Rust, ABI*
  - В `Config` добавлено одно поле: `tls_config: Option<TlsConfig>`. `None` отключает TLS; `Some(TlsConfig { tls_certificate_path, tls_key_path, tls_handshake_timeout })` включает TLS.
  - В C ABI добавлена структура `MMA_TLSConfig` (путь к сертификату, путь к приватному ключу и таймаут рукопожатия в миллисекундах) и одно поле `const MMA_TLSConfig *tls_config` в конце `MMA_ServerConfig`. Размер Rust-структуры `MMA_ServerConfig` с `repr(C)` на x86-64 — 72 байта (вместо 64); размер `MMA_TLSConfig` — 24 байта.
  - Соединение, не прошедшее рукопожатие или не завершившее его за `tls_handshake_timeout`, закрывается и попадает в лог.
  - `ServerHandle::change_config` (и `mma_server_handle_change_config`) отклоняет изменение любого поля TLS: `InvalidInput` в Rust, `MMA_SERVER_ERROR` в C.
- Четыре запускаемых примера сервера: `examples/example_no_tls.rs` и `examples/example_with_tls.rs` для Rust, а также `examples/example_no_tls.c` и `examples/example_with_tls.c` для C ABI. TLS-примеры принимают пути к сертификату и приватному ключу аргументами командной строки. Сертификаты и приватные ключи в репозиторий не включены.
- В `mma_gui_client.py` появился раздел TLS (транспортная защита, проверка сертификата, имя сервера).

### Изменено

- **Rust API (breaking).**
  - `Config`, `FramerConfig`, `BigBytesConfig`, `ConfigError` и `MMA_VERSION` перенесены из `MMA::core::config` в `MMA::config::config`.
  - `ServerHandle` перенесён из `core::server` в `core::handle`. `ChannelHub` теперь приватный; в 0.1.4 он был доступен через `core::server`.
  - `SocketStatus` перенесён из приватного сетевого модуля в `core::commands` и стал публичным.
  - В `Config` четыре новых поля, поэтому литералы структуры без `..Config::default()` больше не компилируются.
  - `examples/main_server.rs` заменён Cargo-примерами `no_tls` и `with_tls`; `cargo run --example main_server` больше не работает. C-примеры находятся в `examples/example_no_tls.c` и `examples/example_with_tls.c` (прежняя ссылка на `examples/main_server.c` устарела).
- `serve_connection` обобщён по типу потока (`AsyncConnection`: `AsyncRead + AsyncWrite + Unpin + Send`), поэтому TLS и обычный TCP используют один путь кода. Внутреннее изменение.
- README: TLS, актуальные имена и команды сборки Rust/C-примеров, все четыре native release target, новые пути модулей, 72-байтная `MMA_ServerConfig`, правило про SIGPIPE и поведение дубликата route в C API. Прежнее утверждение, что существует `FramerConfig::new()`, было ошибочным; конструктор — `FramerConfig::default()` (`BigBytesConfig::new()` существует).
- **CI и native release pipeline.** При push, pull request и ручном запуске CI выполняет `cargo test --locked --all-targets`, собирает release-библиотеку и оба Rust-примера, компилирует оба C-примера с C11 и `-Wall -Wextra -Werror`, а также проверяет совместимость `mma.h` с C++17. Для тегов `v*` матрица релизной сборки создаёт пакеты Windows x86-64, Linux x86-64, macOS Intel x86-64 и macOS Apple Silicon arm64. Каждый пакет содержит native-библиотеку и `mma.h`; пакет Windows дополнительно содержит import library MSVC. Для архивов создаются SHA-256-файлы, которые проверяются перед публикацией GitHub Release. *Только CI/release; изменений Wire, Rust API и ABI нет.*

### Известные проблемы

Проверено запуском кода (`cargo build --release`, `cargo test --locked --all-targets`: 14 тестов проходят; живые сессии с `examples/with_tls`, `examples/no_tls` и C-программой, покрывающей все 20 экспортируемых функций).

1. **Ошибки инициализации TLS теперь возвращаются явно.** `Server::from_listener` возвращает `io::Error`, если цепочку сертификатов или приватный ключ не удалось загрузить; C API возвращает `MMA_SERVER_ERROR` из `mma_server_start`. После ошибки инициализации TLS сервер больше не переключается на обычный TCP. *Исправлено в текущем дереве исходников.*
2. **В `include/mma.h` тип `MMA_TLSConfig` используется до объявления typedef.** `MMA_ServerConfig` содержит `const MMA_TLSConfig *tls_config`, но typedef находится ниже структуры, а предварительного объявления нет. В текущем виде подключение заголовка завершается ошибкой и для C, и для C++; необходимо перенести определение `MMA_TLSConfig` выше `MMA_ServerConfig` либо добавить корректное предварительное объявление именованной структуры. Комментарий над полем также упоминает уже удалённый флаг `tls_enable`: теперь TLS включается наличием ненулевого `tls_config`. C-компиляция примеров и C++ header smoke test в CI будут завершаться ошибкой до исправления заголовка.
3. **C-хост убивается сигналом `SIGPIPE`.** Запись в сокет, у которого закрыт собеседник, вызывает `SIGPIPE`; C-хост его не игнорирует (Rust-бинарник игнорирует). Подтверждено: процесс-хост завершился со статусом 13, когда отключился клиент с полной очередью push. Обход: `signal(SIGPIPE, SIG_IGN)` до `mma_server_start`. Предлагаемое исправление: игнорировать сигнал внутри библиотеки или отправлять с `MSG_NOSIGNAL`.
4. В C API дубликат route проходит `mma_server_register_route` (возвращает `MMA_OK`) и падает только в `mma_server_start` с `MMA_SERVER_ERROR`; причина идёт в stderr.
5. `Config::validate` не проверяет поля TLS.
6. Комментарий в заголовке говорил, что callbacks выполняются «на worker-потоках рантайма»; на деле они идут в blocking-пуле Tokio (исправлено в приложенном заголовке).
7. Автотестов на TLS, `ChannelHub`, хуки жизненного цикла и C ABI нет.
8. Тестовые TLS credentials не входят в репозиторий. Для запуска обоих TLS-примеров предоставьте сертификат и соответствующий приватный ключ; храните приватные ключи вне репозитория и исключайте их из системы контроля версий.
9. Оставшиеся известные проблемы 0.1.3 и 0.1.4: пользовательский disconnect-хук не вызывается при обрыве TCP; `send_to_channel(0, ..)` блокируется одним не читающим клиентом; нет лимита запросов на соединение.
10. Предупреждения компилятора: имя crate `MMA` не в snake case, варианты `ReservedRoutes` в верхнем регистре, два неиспользуемых импорта.
11. Исходники в CRLF; приложен `.gitattributes` с `* text=auto eol=lf`.

## [0.1.4] - 2026-11-07

Тег `v0.1.4` (коммит `b6b8579`). Изменения после `v0.1.3` (коммит `95a5abb`). `MMA_VERSION` остаётся `1`; в `Cargo.toml` по-прежнему `0.1.0`.

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

### Известные проблемы

1. Клиент, не читающий данные, останавливает `send_to_channel(0, ..)` для всех (см. *Изменено*). Для рассылки недоверенным клиентам используйте `try_send_to_channel` или `tokio::time::timeout`.
2. `send_to_channel(0, ..)` останавливается на первом неудачном соединении; следующие за ним сообщение не получают.
3. Пользовательский disconnect-хук вызывается только при явном `CHANNEL_DISCONNECT`, а не при обрыве TCP-соединения. Сообщение, которое хук отправляет в своё же соединение, теряется.
4. Connect-хук выполняется, пока удерживается разрешение состояния соединения, поэтому медленный хук задерживает следующие кадры этого соединения.
5. Одно соединение может занять весь бюджет `max_in_flight` (лимита на соединение нет).
6. Нет автотестов для hub, хуков жизненного цикла и гонки состояния. Существующие 14 тестов проходят. Проверено вручную: `cargo build --release`, `cargo test --locked --all-targets`, C-пример компилируется с `-std=c11 -Wall -Wextra -Werror`, живая сессия с `examples/main_server` (неверный route → `BadRequest`; push connect-хука; повторный connect → `Conflict`; `CHANNEL_DISCONNECT` → соединение закрыто без ответа; `CHANNEL_DISCONNECT` на новом соединении → `BadRequest`; тест с молчащим клиентом выше).
7. В `Cargo.toml` `version = "0.1.0"`, а теги `v0.1.0`–`v0.1.3` существуют. Теги `v0.1.0`–`v0.1.2` находятся на отдельной ветви истории (Go-биндинги) и не являются предками `master`.
8. 17 файлов исходников и workflow в CRLF. Добавьте `.gitattributes` (`* text=auto eol=lf`), чтобы диффы оставались читаемыми.

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
# Changelog

[English](CHANGELOG.md) · **Русский**

Здесь фиксируются все заметные изменения MMA. Формат — [Keep a Changelog](https://keepachangelog.com/ru/1.1.0/). Версии — по [Semantic Versioning](https://semver.org/lang/ru/); до `1.0.0` несовместимое изменение повышает минорную версию.

В каждой записи влияние на три поверхности совместимости указано отдельно (см. [README §9](README.ru.md#9-совместимость-и-версионирование)):

| Метка | Поверхность | Что это |
|---|---|---|
| **Wire** | раскладка кадра и `MMA_VERSION` | байты в соединении |
| **Rust** | crate `MMA` (`rlib`) | публичные элементы `src/` |
| **ABI** | `libMMA` + `include/mma.h` | экспортируемые символы, раскладки структур, контракты callback-ов |

## [Unreleased]

База: коммит `432905c` («added README»). Предлагаемая версия: **0.2.0**, так как изменилась семантика запросов (см. *Изменено → Wire*). В `Cargo.toml` пока `0.1.0`, `MMA_VERSION` по-прежнему `1`.

### Добавлено

- **Хуки жизненного цикла соединения** — пользовательский код при открытии канала и при его закрытии клиентом. *Rust, ABI*
  - Rust: `Router::register_on_channel_connect_handler(f)` и `Router::register_on_channel_disconnect_handler(f)`, где `f: Fn(Request, u32) -> impl Future<Output = ()>`. Повторный вызов заменяет хук. Методы вызова `Router::on_channel_connect` / `on_channel_disconnect` публичные.
  - C: `mma_server_register_on_channel_connect_handler` и `mma_server_register_on_channel_disconnect_handler` (сигнатура как у `mma_server_register_channel_handler`, без route). Добавлены в `mma.h` и экспортируются `libMMA`; эти два символа — единственные добавления ABI.
- **Зарезервированные route** `CHANNEL_CONNECT` и `CHANNEL_DISCONNECT`. `router::ReservedRoutes` стал `pub`, получил вариант `CHANNEL_DISCONNECT` и реализует `Display`. *Rust, Wire*
- `Request` реализует `Clone`. *Rust*
- `examples/main_server.rs` показывает оба хука (они рассылают во все каналы сообщения `Connect: …` / `Disconnect: …`).
- `mma_gui_client.py`: тестовый клиент на Tkinter, только стандартная библиотека Python. Принимает полную раскладку (`FramerConfig`), отправляет все три opcode запроса, декодирует ответы, есть самотест кодека.
- README (`README.md`, `README.ru.md`): §5.5 (зарезервированные route и хуки), примеры хуков для Rust и C, новые C-функции, обновлённые правила потоков, ограничения и структура репозитория. `CHANGELOG.md` и `CHANGELOG.ru.md`.

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

### Известные проблемы этого изменения

Они есть в текущем рабочем дереве и описаны в README §11. Здесь не исправлены.

1. `ChannelConnect` выполняет замыкание connect (оно создаёт подписку на канал) **до** проверки route, поэтому неверный route может оставить неиспользуемую подписку. Найдено чтением кода, не измерено.
2. После disconnect-хука кадр всё равно идёт в маршрутизацию channel-обработчиков. Без обработчика `CHANNEL_DISCONNECT` клиент перед закрытием соединения получает `NotFound` («No such method»). Подтверждено живым тестом.
3. Обрыв TCP-соединения disconnect-хук не запускает; только явный кадр `CHANNEL_DISCONNECT`.
4. Зарезервированные route сравниваются через `to_uppercase()` на каждом запросе (без учёта регистра, с аллокацией). Пустой `if` в ветке `Fresh`/`ChannelMessage` — мёртвый код.
5. Хуки и проверки зарезервированных route не покрыты автотестами. Проверено вручную: `cargo build --release`, `cargo test` (14 тестов проходят, без изменений), C-пример компилируется с новым заголовком, все 20 функций `mma_*` из заголовка экспортируются, живая сессия с `examples/main_server` (неверный route → `BadRequest`; connect-хук → push `Connect: …`; повторный connect → `Conflict`; disconnect → push хука, `NotFound`, закрытие через ≈ 4 с при `drain_timeout` 2 с; `CHANNEL_DISCONNECT` на новом соединении → `BadRequest`).
6. Переводы строк: 18 файлов рабочего дерева в CRLF (изменение без содержимого относительно коммита). Перед коммитом добавьте `.gitattributes` (`* text=auto eol=lf`), чтобы дифф оставался читаемым.

[Unreleased]: https://github.com/Edestuss0/MMA-Network-protocol/compare/432905c...HEAD

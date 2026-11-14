<div align="center">

# MMA Network Protocol

**Бинарный TCP-протокол с настраиваемым wire-форматом — ядро на Rust, C ABI.**

[English](README.md) · **Русский**

</div>


MMA — приватный бинарный протокол поверх TCP с режимами запрос/ответ и channel. Главная особенность: **единого фиксированного wire-формата нет.** Раскладка байтов кадра определяется конфигурацией (`FramerConfig`), общей для обеих сторон, поэтому два развёртывания с разными конфигурациями выглядят на проводе как два разных несовместимых протокола.

Документ описывает формат кадра, модель соединения, Rust API и C ABI.

## Содержание

- [1. Обзор](#1-обзор)
- [2. Быстрый старт](#2-быстрый-старт)
- [3. Формат кадра](#3-формат-кадра)
- [4. Options](#4-options)
- [5. Маршруты и модель соединения](#5-маршруты-и-модель-соединения)
- [6. Ошибки](#6-ошибки)
- [7. Rust API](#7-rust-api)
- [8. C ABI](#8-c-abi)
- [9. Совместимость и версионирование](#9-совместимость-и-версионирование)
- [10. Безопасность](#10-безопасность)
- [11. Структура репозитория](#11-структура-репозитория)

---

## 1. Обзор

### 1.1. Идея: wire-формат без единого стандарта

У большинства протоколов (HTTP, RESP, MQTT, gRPC) раскладка байтов зафиксирована спецификацией, поэтому любой сниффер опознаёт их по сигнатуре. MMA задаёт **правила, а не раскладку**. Раскладка выбирается для каждого развёртывания:

| Поле `FramerConfig` | Что задаёт |
|---|---|
| `header_length` | размер заголовка, 17–255 байт |
| `opcode_pos`, `version_pos`, `route_len_pos`, `options_count_pos` | в каком байте заголовка лежит каждое однобайтовое поле |
| `big_bytes_config` | какой из трёх 4-байтовых слотов в конце заголовка хранит `options_len`, `payload_len`, `req_id` |
| `route_order`, `options_order`, `payload_order` | порядок секций тела (перестановка 1..3) |
| `options_key_first` | порядок key/value внутри option |
| spare-байты заголовка | нули при encode, игнорируются при decode |

Следствия:

- В кадре нет постоянной сигнатуры (magic number); позиции полей и порядок секций различаются между развёртываниями. Это усложняет распознавание по простому шаблону, но это **обфускация, а не механизм безопасности**.
- Конфигурация — **общий wire-профиль**, который клиент и сервер согласуют вне протокола. Значения на обеих сторонах должны совпадать.
- Неизменна **логическая модель**: `opcode`, `version`, `req_id`, `route`, `options`, `payload`.
- MMA **не шифрует и не аутентифицирует**. Используйте TLS или доверенную сеть (см. [Безопасность](#10-безопасность)).
- Рукопожатия пока нет: при разных конфигурациях сервер неверно разбирает кадры — обычно отвечает `BadRequest` и закрывает соединение.

### 1.2. Назначение

MMA предназначен для связи собственных клиентов и серверов, когда нужны:

- компактный кадр с явными длинами;
- wire-формат, настраиваемый под развёртывание;
- два режима — **Once** (запрос → один ответ) и **Channel** (постоянное соединение с push от сервера);
- один серверный движок для Rust и C.

Вне задач протокола: шифрование, аутентификация, service discovery, совместимость со сторонними клиентами, гарантии доставки между перезапусками. Клиент в crate **не входит** — пишите его по этой спецификации.

### 1.3. Свойства

| Свойство | Значение |
|---|---|
| Транспорт | TCP |
| Взаимодействие | Request/Response (`Once`) и `Channel` |
| Кодирование | бинарное, раскладка задаётся `FramerConfig` |
| Версия wire | `MMA_VERSION = 1` |
| Размер заголовка | минимум 17 байт, по умолчанию 29 |
| Максимальный кадр | `header_length + max_message_length` (по умолчанию `29 + 10 000 000`) |
| Порядок байтов | big-endian |
| Интерфейсы | Rust (серверный API), C (`include/mma.h`) |

---

## 2. Быстрый старт

### Rust-сервер

```toml
[dependencies]
MMA = { git = "https://github.com/Edestuss0/MMA-Network-protocol" }
tokio = { version = "1", features = ["full"] }
bytes = "1"
```

```rust
use MMA::config::config::Config;
use MMA::core::{commands::{Request, Response}, router::Router, server::Server};
use bytes::Bytes;

async fn upper(req: Request, mut res: Response) -> Response {
    res.payload = Bytes::from(req.payload.to_ascii_uppercase());
    res
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let mut server = Server::new(Config::default()).await?;
    let mut router = Router::new();
    router.register_route("UP".to_string(), upper).expect("route");
    server.run(router).await
}
```

В репозитории есть два Rust-примера. Обычный TCP-сервер запускается командой `cargo run --locked --example no_tls`. TLS-сервер принимает пути к PEM-сертификату и приватному ключу: `cargo run --locked --example with_tls -- path/to/server.crt path/to/server.key`. Оба примера слушают `0.0.0.0:8080`. Репозиторий не содержит сертификатов или приватных ключей: для тестирования и развёртывания предоставьте собственные файлы.

### Минимальный клиент (Python, раскладка по умолчанию)

```python
import socket, struct

HEADER_LEN = 29

def recv_exact(sock, size):
    buf = bytearray()
    while len(buf) < size:
        part = sock.recv(size - len(buf))
        if not part:
            raise EOFError("connection closed")
        buf.extend(part)
    return bytes(buf)

def make_frame(opcode, route, payload=b"", req_id=1):
    h = bytearray(HEADER_LEN)
    h[0], h[1], h[2], h[3] = opcode, 1, len(route), 0   # opcode, version, route_len, options_count
    h[17:29] = struct.pack(">III", 0, len(payload), req_id)  # options_len, payload_len, req_id
    return bytes(h) + route + payload

with socket.create_connection(("127.0.0.1", 8080)) as s:
    s.sendall(make_frame(2, b"UP", b"hello", req_id=1))
    h = recv_exact(s, HEADER_LEN)
    payload_len = struct.unpack(">I", h[21:25])[0]
    print(h[0], recv_exact(s, payload_len))   # 1 b'HELLO'
```

### Примеры на C

В репозитории есть две реализации сервера на C: `example_no_tls.c` (обычный TCP) и `example_with_tls.c` (TLS). На Linux соберите shared library и оба примера компилятором с поддержкой C11:

```bash
cargo build --locked --release --lib

for source in examples/example_no_tls.c examples/example_with_tls.c; do
    name="$(basename "${source%.c}")"
    cc \
        -std=c11 \
        -D_POSIX_C_SOURCE=200809L \
        -Wall -Wextra -Wpedantic -Werror \
        -I include \
        "$source" \
        -L target/release \
        -Wl,-rpath,"$PWD/target/release" \
        -lMMA -pthread \
        -o "target/$name"
done
```

Обычный TCP-пример запускается через `./target/example_no_tls`. TLS-примеру нужны пути к сертификату и приватному ключу: `./target/example_with_tls path/to/server.crt path/to/server.key`. Команды выше используют Linux/GNU linker; для Windows и macOS примените соответствующие параметры компилятора и поиска динамической библиотеки.

Обе C-программы используют публичный заголовок [`include/mma.h`](include/mma.h). См. [8. C ABI](#8-c-abi).

---

## 3. Формат кадра

### 3.1. Схема

```text
┌──────────────────────────────────────────────────────────────┐
│ Header — header_length байт (≥ 17, по умолчанию 29)          │
│   opcode / version / route_len / options_count               │
│       (по одному байту, на настроенных позициях)             │
│   spare-байты (нули)                                         │
│   последние 12 байт: три 4-байтовых big-endian слота с       │
│       options_len, payload_len, req_id                       │
│       (порядок слотов задаёт big_bytes_config)               │
├──────────────────────────────────────────────────────────────┤
│ Body — три секции в порядке route_order / options_order /    │
│        payload_order                                         │
│   Route    route_len байт (UTF-8)                            │
│   Options  options_len байт                                  │
│   Payload  payload_len байт                                  │
└──────────────────────────────────────────────────────────────┘
```

В ответах секции route нет (`route_len = 0`).

### 3.2. Поля заголовка

Смещения — от начала кадра, с нуля; `H = header_length`. Позиции `*_pos` в конфигурации считаются с единицы, поэтому смещение = `pos − 1`. Смещения трёх 4-байтовых полей указаны для порядка слотов по умолчанию.

| Поле | Смещение | Размер | Тип | Назначение | Проверка |
|---|---:|---:|---|---|---|
| `opcode` | `opcode_pos − 1` (по ум. 0) | 1 | `u8` | Тип запроса или ответа ([3.4](#34-opcode)) | запрос: 1, 2 или 3 |
| `version` | `version_pos − 1` (по ум. 1) | 1 | `u8` | Версия протокола, сейчас `1` | проверяет роутер |
| `route_len` | `route_len_pos − 1` (по ум. 2) | 1 | `u8` | Длина route; в ответах `0` | 0–255, route — UTF-8 |
| `options_count` | `options_count_pos − 1` (по ум. 3) | 1 | `u8` | Число options | 0–255, совпадает с разобранным |
| spare | остальное в `[0, H−12)` | `H − 16` | — | Резерв для вариации раскладки | нули при encode, игнорируются при decode |
| `options_len` | `H − 12` (по ум.) | 4 | `u32` BE | Размер секции options | входит в `max_message_length` |
| `payload_len` | `H − 8` (по ум.) | 4 | `u32` BE | Размер payload | входит в `max_message_length` |
| `req_id` | `H − 4` (по ум.) | 4 | `u32` BE | Идентификатор запроса, возвращается в ответе | любое `u32` |

Зачем нужно каждое поле:

- **`opcode`** — что это за кадр и как его обрабатывать.
- **`version`** — версия протокола; позволяет менять семантику без смены раскладки.
- **`route_len`** — граница секции route; route выбирает обработчик на сервере.
- **`options_count`** — число пар key/value; вместе с `options_len` проверяет целостность секции.
- **`options_len`, `payload_len`** — размеры секций; по ним вычисляется длина кадра.
- **`req_id`** — связывает ответ с запросом. Ответы могут приходить не по порядку, поэтому `req_id` среди запросов в полёте должны быть уникальны.
- **spare-байты** — свободное место заголовка для вариации раскладки; смысла не несут.

### 3.3. Длина кадра

```text
body_len  = route_len + options_len + payload_len    (checked-сложение)
frame_len = header_length + body_len                 (checked-сложение)
```

`body_len` сравнивается с `max_message_length` до любой аллокации. Кадр может прийти за несколько чтений TCP, несколько кадров — за одно; границы определяются только полями длины.

### 3.4. Opcode

**Запросы (клиент → сервер)**

| Название | Код | Допустимо в состоянии | Назначение |
|---|---:|---|---|
| `ChannelConnect` | 1 | `Fresh` | Перевести соединение в режим Channel; route **обязан** быть `CHANNEL_CONNECT` ([§5.5](#55-зарезервированные-маршруты-и-хуки-жизненного-цикла-соединения)) |
| `Once` | 2 | `Fresh`, `Once` | Запрос обрабатывается route-обработчиком; один ответ |
| `ChannelMessage` | 3 | `Channel` | Сообщение channel-обработчику; при успехе ответа нет. Route `CHANNEL_DISCONNECT` зарезервирован ([§5.5](#55-зарезервированные-маршруты-и-хуки-жизненного-цикла-соединения)) |

**Ответы (сервер → клиент)**

| Название | Код | Значение | Когда отправляется |
|---|---:|---|---|
| `Ok` | 1 | Успех | Статус ответа по умолчанию |
| `BadRequest` | 2 | Некорректный запрос | Ошибка декодирования; неверная версия; channel-сообщение без канала; `ChannelConnect` с route не `CHANNEL_CONNECT` |
| `Unauthorized` | 3 | Не авторизован | Выставляет ваш обработчик |
| `Forbidden` | 4 | Запрещено | Выставляет ваш обработчик |
| `NotFound` | 5 | Route не найден | payload `"No such method"` |
| `Conflict` | 6 | Конфликт состояния | Повторный `ChannelConnect` |
| `InternalError` | 7 | Ошибка сервера | Panic обработчика |
| `ChannelMessage` | 8 | Push от сервера | Доставка через `send_to_channel` / `try_send_to_channel` |
| `Shutdown` | 9 | Сервер останавливается | Зарезервирован; сейчас не отправляется |
| `ChannelConnected` | 10 | Канал установлен | Ответ на `ChannelConnect` |
| `Timeout` | 11 | Таймаут обработчика | Истёк `request_timeout` |

Коды `0` и `≥ 12` не определены. Неизвестный opcode запроса — ошибка декодирования.

### 3.5. Примеры кадров

Раскладка по умолчанию (`header_length = 29`, позиции 1/2/3/4, порядок route → options → payload).

Запрос: `Once`, route `GET`, payload `hello`, `req_id = 1` — 37 байт:

```text
02 01 03 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 05 00 00 00 01 47 45 54 68 65 6c 6c 6f
```

| Смещение | Hex | Поле | Значение |
|---:|---|---|---|
| 0 | `02` | `opcode` | `Once` |
| 1 | `01` | `version` | 1 |
| 2 | `03` | `route_len` | 3 |
| 3 | `00` | `options_count` | 0 |
| 4–16 | `00 …` | spare | 13 нулевых байт |
| 17–20 | `00 00 00 00` | `options_len` | 0 |
| 21–24 | `00 00 00 05` | `payload_len` | 5 |
| 25–28 | `00 00 00 01` | `req_id` | 1 |
| 29–31 | `47 45 54` | route | `"GET"` |
| 32–36 | `68 65 6c 6c 6f` | payload | `"hello"` |

Ответ `Ok` — 34 байта:

```text
01 01 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 05 00 00 00 01 68 65 6c 6c 6f
```

Тот же логический запрос при другой конфигурации (например, `version_pos=4`, `route_len_pos=6`, `options_count_pos=7`, порядок payload → options → route, как в `examples/example_no_tls.rs`) даст совершенно другие байты. В этом и состоит идея.

### 3.6. Конфигурация раскладки

| Поле | Допустимые значения | По умолчанию | Влияние на wire |
|---|---|---|---|
| `header_length` | 17–255 | 29 | размер заголовка, положение 12-байтового хвоста |
| `opcode_pos`, `version_pos`, `route_len_pos`, `options_count_pos` | 1…`header_length − 12`, попарно различны | 1, 2, 3, 4 | смещения однобайтовых полей |
| `route_order`, `options_order`, `payload_order` | перестановка 1..3 | 1, 2, 3 | порядок секций тела |
| `big_bytes_config.options_len_order` / `payload_len_order` / `req_id_order` | перестановка 1..3 | 1, 2, 3 | слот каждого 4-байтового поля в хвосте |
| `options_key_first` | `true` / `false` | `true` | `key,value` или `value,key` внутри option |
| `max_message_length` | `u32` | 10 000 000 | не параметр раскладки; серверный лимит размера |

Все поля, кроме `max_message_length`, — параметры wire-формата и **должны совпадать на обеих сторонах**.

### 3.7. Неполные и некорректные кадры

| Ситуация | Реакция сервера | Соединение |
|---|---|---|
| Неполный заголовок или тело | ждёт данных | открыто |
| EOF посреди кадра | ответа нет | закрывается |
| Длина больше `max_message_length` | `BadRequest` | закрывается |
| Переполнение длины | `BadRequest` | закрывается |
| Неизвестный opcode запроса | `BadRequest` | закрывается |
| Route — не валидный UTF-8 | `BadRequest` | закрывается |
| Некорректные options (пустой key/value, усечённые данные, лишние байты, неверное число) | `BadRequest` | закрывается |
| Неверная `version` | `BadRequest` (`"Incorrect protocol version"`) | остаётся открытым |

После ошибки декодирования поток нельзя пересинхронизировать, поэтому соединение закрывается. `BadRequest` содержит `req_id` из заголовка, если его удалось прочитать, иначе `0`.

---

## 4. Options

Options — именованные параметры кадра (метаданные, флаги, trace-идентификаторы). Протокол не резервирует ключи; смысл определяет приложение.

| Свойство | Поведение |
|---|---|
| Ключ | произвольные байты, чувствителен к регистру, 1–65535 байт |
| Значение | произвольные байты, 1–65535 байт |
| Пустой ключ или значение | отклоняется |
| Повторяющиеся ключи | разрешены; порядок сохраняется |
| Количество | 0–255 на кадр |
| Неизвестные ключи | библиотека игнорирует; доступны в `Request.options` |

Wire-формат одной option (`options_count` штук подряд образуют секцию options):

```text
options_key_first = true :  [key_len u16 BE][value_len u16 BE][key][value]
options_key_first = false:  [key_len u16 BE][value_len u16 BE][value][key]
```

Пример — `POW`, payload `7`, option `pow = 3`, `req_id = 2`, раскладка по умолчанию (41 байт):

```text
02 01 03 01 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 08 00 00 00 01 00 00 00 02 50 4f 57 00 03 00 01 70 6f 77 33 37
```

| Секция | Байты | Значение |
|---|---|---|
| route (3) | `50 4f 57` | `"POW"` |
| options (8) | `00 03` `00 01` `70 6f 77` `33` | `key_len=3`, `value_len=1`, ключ `pow`, значение `3` |
| payload (1) | `37` | `"7"` |

Заголовок: `options_count = 1`, `options_len = 8`, `payload_len = 1`, `req_id = 2`.

Доступ из Rust: `Request::get_option(&self, key: &[u8]) -> Option<Bytes>` (данные не копируются). Значения — сырые байты: разбирайте их сами и **не используйте `unwrap` на значениях от клиента**. Типизированных геттеров нет.

---

## 5. Маршруты и модель соединения

### 5.1. Маршруты

- Route — UTF-8 строка до 255 байт, чувствительна к регистру.
- Неизвестный route → `NotFound` (`"No such method"`); соединение остаётся открытым.
- Повторная регистрация того же route (в том числе между request- и channel-обработчиками) — ошибка.
- Два имени route **зарезервированы** под жизненный цикл канала: `CHANNEL_CONNECT` и `CHANNEL_DISCONNECT` ([§5.5](#55-зарезервированные-маршруты-и-хуки-жизненного-цикла-соединения)). Сравниваются без учёта регистра.

### 5.2. Состояния соединения

```text
            ChannelConnect                     ChannelConnect
 Fresh ──────────────────────► Channel ◄──────────────────────┐ (Conflict, остаётся)
   │                             │  ▲                          │
   │ Once                        │  └── ChannelMessage ───────┘ (ответа нет)
   ▼                             │
 Once ◄── Once (цикл)            └── Once → mismatch (остаётся)
```

| Состояние | Запрос | Результат | Новое состояние |
|---|---|---|---|
| `Fresh` | `Once` | route-обработчик | `Once` |
| `Fresh` | `ChannelConnect`, route `CHANNEL_CONNECT` | `ChannelConnected`; запускается connect-хук | `Channel` |
| `Fresh` | `ChannelConnect`, любой другой route | `BadRequest` («Route must match the opcode») | `Fresh` |
| `Fresh` | `ChannelMessage` | `BadRequest` («Channel not connected») | `Fresh` |
| `Once` | `Once` | route-обработчик | `Once` |
| `Once` | `ChannelConnect` / `ChannelMessage` | `BadRequest` (несовпадение типа) | `Once` |
| `Channel` | `ChannelConnect` | `Conflict` | `Channel` |
| `Channel` | `ChannelMessage` | channel-обработчик | `Channel` |
| `Channel` | `ChannelMessage`, route `CHANNEL_DISCONNECT` | запускается disconnect-хук, канал снимается с регистрации, соединение закрывается, ответа нет; см. [§5.5](#55-зарезервированные-маршруты-и-хуки-жизненного-цикла-соединения) | закрывается |
| `Channel` | `Once` | `BadRequest` (несовпадение типа) | `Channel` |

Используйте один режим на соединение и не отправляйте `Once` и `ChannelConnect` одновременно на новом соединении.

### 5.3. Запрос/ответ (Once)

- `req_id` выбирает клиент; сервер возвращает его без изменений.
- По одному соединению может идти много запросов. Единственный предел — `max_in_flight` на весь сервер; лимита на соединение нет.
- **Ответы могут приходить не в порядке запросов.** Сопоставляйте по `req_id`.
- Если обработчик превысил `request_timeout`, сервер отвечает `Timeout` (11). Отмена срабатывает на ближайшем `.await`; блокирующий код не прерывается.
- Panic обработчика перехватывается и превращается в `InternalError`; соединение остаётся открытым.
- Повтор запроса после таймаута или разрыва — на усмотрение приложения; протокол не гарантирует идемпотентность.

### 5.4. Режим Channel

- `ChannelConnect` переводит соединение в режим Channel; сервер присваивает идентификатор соединения (`conn_id`) и не сообщает его клиенту.
- Сервер отправляет push через `ServerHandle`:

  | Метод | Ждёт? | Ошибка |
  |---|---|---|
  | `send_to_channel(conn_id, Response).await` | да — пока в очереди получателя не появится место | `Err(String)` |
  | `try_send_to_channel(conn_id, Response)` | нет | `Err(String)`, если очередь полна или канала нет |

  **`conn_id = 0` — рассылка всем channel-соединениям.** Channel-обработчик получает `conn_id` соединения-отправителя. `opcode` отправляемого ответа нужно выставить в `ChannelMessage` (8) самостоятельно.
- У каждого channel-соединения своя очередь на **256 сообщений**. Сообщение для одного `conn_id` идёт сразу в это соединение (поиск по хеш-таблице), а не всем. Неизвестный `conn_id` даёт `Err("No such channel")`. Рассылка при отсутствии channel-соединений — `Ok`.
- `Ok(())` означает «принято в очередь получателя», а не «доставлено». Сообщения не теряются молча: при полной очереди `send_to_channel` ждёт, а `try_send_to_channel` возвращает ошибку.
- **Получатель, который не читает, блокирует рассылку.** `send_to_channel(0, ..)` обходит соединения по очереди и ждёт на полной очереди, поэтому один зависший клиент задерживает всех, кто идёт после него (замер на loopback: при одном молчащем клиенте и сообщениях по 64 КиБ другой клиент перестал получать после ≈ 1300 из 3000). При первой ошибке `send_to_channel(0, ..)` останавливается, и остальные соединения это сообщение не получают; `try_send_to_channel(0, ..)` пробует всех и возвращает `Partial broadcast: N/M channels accepted the message`. Для рассылки недоверенным клиентам используйте `try_send_to_channel` или оборачивайте `send_to_channel` в `tokio::time::timeout`.
- Push-сообщения и обычные ответы идут через одну исходящую очередь; их взаимный порядок не гарантируется.

### 5.5. Зарезервированные маршруты и хуки жизненного цикла соединения

Два имени route несут жизненный цикл канала. Оба сравниваются без учёта регистра.

| Route | Opcode | Направление | Эффект |
|---|---|---|---|
| `CHANNEL_CONNECT` | `ChannelConnect` (1) | клиент → сервер | открывает режим Channel; затем сервер запускает **connect-хук** с запросом (доступны payload и options) и новым `conn_id` |
| `CHANNEL_DISCONNECT` | `ChannelMessage` (3) | клиент → сервер | запускает **disconnect-хук** с запросом и `conn_id`, затем сервер перестаёт читать это соединение и закрывает его |

Поведение:

- `ChannelConnect` с любым другим route возвращает `BadRequest`. Клиенты, написанные под прежние сборки и отправлявшие произвольный route, теперь должны отправлять `CHANNEL_CONNECT`.
- `CHANNEL_DISCONNECT` учитывается только в состоянии `Channel`; иначе ответ — `BadRequest` («Channel not connected»).
- Хуки необязательны. Без зарегистрированного хука служебный route работает, но пользовательский код не вызывается.
- Хук получает копию `Request` клиентского кадра; он ничего не возвращает и не может отклонить соединение.
- `CHANNEL_DISCONNECT` не даёт ответа: после хука сервер снимает канал с регистрации и закрывает соединение. В channel-обработчики кадр не попадает. Сообщение, которое хук отправляет в это же соединение, теряется: канал уже закрывается.
- Panic в connect-хуке перехватывается: клиент получает `InternalError`, регистрация канала откатывается, соединение остаётся `Fresh`. Panic в disconnect-хуке даёт `InternalError`, соединение не закрывается.
- Ответ на `ChannelConnect` с неверным route — `BadRequest` с payload `"Route must match the opcode"`. Канал регистрируется только после проверки route.
- Обрыв TCP-соединения (EOF, reset) снимает канал с регистрации, но пользовательский disconnect-хук **не** запускает. Не полагайтесь только на хук для освобождения состояния соединения.
- Смена состояния одного соединения решается по одному кадру за раз (семафор на одно разрешение). Connect-хук выполняется внутри этого шага, поэтому медленный хук задерживает следующие кадры того же соединения.

---

## 6. Ошибки

| Условие | Ответ | `req_id` | Соединение | Повтор? |
|---|---|---|---|---|
| Кадр слишком большой / некорректный / неизвестный opcode | `BadRequest` | из заголовка, иначе 0 | закрывается | нет — исправить кадр |
| Неверная версия | `BadRequest` | запроса | открыто | нет |
| Неизвестный route | `NotFound` | запроса | открыто | нет |
| Режим не подходит opcode | `BadRequest` | запроса | открыто | нет |
| `ChannelConnect` с route не `CHANNEL_CONNECT` | `BadRequest` | запроса | открыто | нет — использовать зарезервированный route |
| Повторный `ChannelConnect` | `Conflict` | запроса | открыто | нет |
| Panic обработчика | `InternalError` | запроса | открыто | зависит от операции |
| Таймаут обработчика | `Timeout` | запроса | открыто | если идемпотентно |
| Ошибка TCP / EOF | нет | — | закрывается | на новом соединении |

`Unauthorized` и `Forbidden` доступны вашим обработчикам; сама библиотека их не отправляет.

---

## 7. Rust API

Публичные модули: `MMA::config::config` (`Config`, `FramerConfig`, `BigBytesConfig`), `MMA::core::{commands, router, server, handle}` и `MMA::c_api`. Таблица каналов и TLS-акцептор приватны. Framer и сетевой слой приватны. Crate собирается как `rlib`, `cdylib` и `staticlib`.

### 7.1. Типы

| Тип | Роль |
|---|---|
| `Config` | настройки сервера (`Clone`, `Debug`); `Config::default()`, `validate()` |
| `FramerConfig` | wire-раскладка; `FramerConfig::default()`, `validate()` |
| `Request` | `headers`, `payload: Bytes`, `route: String`, `options: Vec<(Bytes, Bytes)>` |
| `Response` | `headers`, `payload: Bytes`, `options`; создаётся `Response::new(req_id)` |
| `Router` | таблица маршрутов |
| `Server` | владеет listener; `run`, `run_until` |
| `ServerHandle` (`core::handle`) | дешёвый `Clone`, `Send + Sync`; отправляет channel-сообщения, меняет конфигурацию |

### 7.2. Конфигурация

```rust
use MMA::config::config::{Config, FramerConfig};
use std::net::SocketAddr;
use std::time::Duration;

let config = Config {
    address: SocketAddr::from(([0, 0, 0, 0], 8080)),
    frame_config: FramerConfig::default(),
    max_batch: 255,
    max_in_flight: 1024,
    max_connections: 128_000,
    drain_timeout: Duration::from_secs(2),
    shutdown_grace: Duration::from_secs(10),
    request_timeout: Duration::from_secs(10),
    tls_config: None,
};
config.validate().expect("invalid config");
```

| Параметр | По умолчанию | Смысл |
|---|---|---|
| `max_connections` | 128 000 | одновременные соединения (≥ 1) |
| `max_in_flight` | 1024 | одновременно выполняемые обработчики на весь сервер (≥ 1) |
| `max_batch` | 255 (Rust), 16 (C) | число ответов, записываемых до одного flush (≥ 1) |
| `max_message_length` (в `FramerConfig`) | 10 000 000 | максимальный размер тела в байтах |
| `request_timeout` | 10 с | лимит времени обработчика |
| `drain_timeout` | 2 с | время на отправку накопленных ответов при закрытии соединения |
| `tls_config` | `None` | необязательная конфигурация TLS-сервера (`TlsConfig`) |
| `shutdown_grace` | 10 с | ожидание выполняющихся задач при остановке до принудительной отмены |

`Config::validate` отклоняет нулевые лимиты, неверный `FramerConfig` и **порт 0**. Загрузка сертификата и ключа выполняется при инициализации сервера, а не внутри `Config::validate`.

#### TLS

MMA может самостоятельно завершать TLS (rustls, без клиентских сертификатов). Для этого задайте `tls_config: Some(TlsConfig { ... })`; значение `None` отключает TLS:

```rust
use MMA::config::config::{Config, TlsConfig};
use std::time::Duration;

let config = Config {
    tls_config: Some(TlsConfig {
        tls_certificate_path: "server.crt".into(), // цепочка сертификатов PEM
        tls_key_path: "server.key".into(),         // приватный ключ PEM
        tls_handshake_timeout: Duration::from_secs(10),
    }),
    ..Config::default()
};
```

- `tls_config: None` отключает TLS. `Some(TlsConfig { ... })` включает его и задаёт путь к сертификату, приватному ключу и таймаут рукопожатия.
- Если сертификат или ключ не удаётся загрузить, инициализация Rust-сервера возвращает `io::Error`; сервер не переключается молча на обычный TCP. В C API ошибка запуска возвращается как `MMA_SERVER_ERROR`. Проверяйте код возврата и лог сервера.
- При включённом TLS клиент без шифрования отклоняется на рукопожатии (`TLS Handshake failed`); рукопожатие дольше `tls_handshake_timeout` обрывается.
- Кадровый протокол внутри TLS-потока не меняется. Клиент самостоятельно проверяет цепочку сертификатов сервера и имя узла. Репозиторий не содержит CA bundle; настройте хранилище доверия в соответствии с предоставленным сертификатом.
- Настройки TLS нельзя менять на работающем сервере: `ServerHandle::change_config` вернёт `InvalidInput` («TLS settings cannot be changed while server is running»), если любое поле TLS отличается.

### 7.3. Обработчики

```rust
async fn echo(req: Request, mut res: Response) -> Response {
    res.payload = req.payload;       // Bytes, без копирования
    res
}

let mut router = Router::new();
router.register_route("GET", echo)?;
```

Обработчик принимает `Request` и `Response` по значению и возвращает `Response`. Статус по умолчанию — `Ok`; меняйте `res.headers.opcode` или используйте `Response::bad_request(req_id, Some(payload))`, `Response::conflict(..)` и т.д.

Channel-обработчик:

```rust
use MMA::core::commands::ResponseOpcode;

router.register_channel_handler("SEND".to_string(), {
    let handle = handle.clone();                      // ServerHandle
    move |req: Request, _conn_id: u32| {
        let handle = handle.clone();
        async move {
            let mut msg = Response::new(0);
            msg.headers.opcode = ResponseOpcode::ChannelMessage;
            msg.payload = req.payload;
            let _ = handle.send_to_channel(0, msg).await;   // 0 = всем
        }
    }
})?;
```

Общее состояние — захватывайте `Arc` в замыкании:

```rust
use std::sync::{Arc, atomic::{AtomicU64, Ordering}};

let counter = Arc::new(AtomicU64::new(0));
router.register_route("COUNT", {
    let counter = counter.clone();
    move |_req, mut res: Response| {
        let counter = counter.clone();
        async move {
            res.payload = Bytes::from((counter.fetch_add(1, Ordering::Relaxed) + 1).to_string());
            res
        }
    }
})?;
```

Хуки жизненного цикла соединения (необязательные, см. [§5.5](#55-зарезервированные-маршруты-и-хуки-жизненного-цикла-соединения)):

```rust
router.register_on_channel_connect_handler({
    let handle = handle.clone();
    move |req: Request, conn_id: u32| {
        let handle = handle.clone();
        async move {
            let mut msg = Response::new(0);
            msg.headers.opcode = ResponseOpcode::ChannelMessage;
            msg.payload = Bytes::from(format!("connected: {conn_id}, payload: {} bytes", req.payload.len()));
            let _ = handle.send_to_channel(0, msg).await;
        }
    }
});

router.register_on_channel_disconnect_handler(|req: Request, conn_id: u32| async move {
    // здесь освободите состояние соединения
});
```

Оба метода имеют сигнатуру `Fn(Request, u32) -> impl Future<Output = ()>` и не возвращают `Result`. Повторная регистрация заменяет прежний хук. Имена зарезервированных route экспортируются как `router::ReservedRoutes` (`CHANNEL_CONNECT`, `CHANNEL_DISCONNECT`). `Request` теперь реализует `Clone`.

Не блокируйте поток в обработчиках: `request_timeout` не отменяет блокирующий код, а он занимает worker рантайма. Для блокирующей и тяжёлой CPU-работы используйте `tokio::task::spawn_blocking`.

### 7.4. Жизненный цикл сервера

| Метод | Поведение |
|---|---|
| `Server::new(config).await` | проверяет конфигурацию и привязывает listener |
| `Server::from_listener(config, listener)` | использует готовый listener (конфигурация всё равно проверяется, порт 0 отклоняется) |
| `server.handle()` | возвращает `ServerHandle` |
| `handle.change_config(config)` | проверяет и публикует новую конфигурацию; поля TLS должны остаться прежними, иначе `Err(io::Error)` |
| `server.run(router).await` | работает до Ctrl+C, затем graceful shutdown |
| `server.run_until(router, fut).await` | работает, пока не завершится `fut` |
| `server.shutdown_token()` / `shutdown_server()` | токен отмены / запуск остановки |
| `server.local_addr()` | адрес, на котором слушает сервер |

Остановка: прекращается приём соединений и чтение, выполняющиеся обработчики получают `shutdown_grace`, затем всё отменяется.

### 7.5. Ошибки

| Вызов | Ошибка | Причина |
|---|---|---|
| `Server::new` | `io::Error` | неверная конфигурация или ошибка bind |
| `Router::register_route` / `register_channel_handler` | `Err` | пустой или слишком длинный route, дубликат |
| `Router::register_on_channel_connect_handler` / `register_on_channel_disconnect_handler` | — | не возвращают ошибок; повторный вызов заменяет хук |
| `ServerHandle::send_to_channel` / `try_send_to_channel` | `Err(String)` | неизвестный `conn_id`, закрытый получатель, полная очередь (только `try_`), частичная рассылка |
| `Config::validate` | `ConfigError` | нулевой лимит, порт 0, неверный `FramerConfig` |
| `Server::run` / `run_until` | `io::Error` | ошибка listener/accept |

---

## 8. C ABI

Публичный интерфейс C определён в [`include/mma.h`](include/mma.h). Текущая версия — **API с непрозрачными handle и callback-функциями**: C-код не выделяет память под `MMA_Server`, `MMA_ServerHandle`, `MMA_Request` или `MMA_Response` и не обращается к их внутренним полям.

Для каждого тега `v*` GitHub Release публикует native-пакеты Windows x86-64, Linux x86-64, macOS Intel x86-64 и macOS Apple Silicon arm64. Каждый архив содержит динамическую библиотеку и `mma.h`; Windows дополнительно содержит `MMA.dll.lib`. Рядом публикуется SHA-256-файл.

### 8.1. Сборка и линковка

Сначала соберите библиотеку:

```bash
cargo build --locked --release --lib
```

Пример для Linux (для TLS замените `example_no_tls.c` на `example_with_tls.c`):

```bash
cc -std=c11 -D_POSIX_C_SOURCE=200809L \
    -Wall -Wextra -Wpedantic -Werror \
    -Iinclude examples/example_no_tls.c \
    -Ltarget/release -Wl,-rpath,"$PWD/target/release" \
    -lMMA -pthread -o basic_server

./basic_server
```

В Windows используйте MSVC target и import library, созданную Cargo. Пример с LLVM Clang, настроенным на MSVC toolchain:

```powershell
cargo build --locked --release --lib

clang --target=x86_64-pc-windows-msvc `
    -fuse-ld=link `
    -std=c11 -O2 `
    -Wall -Wextra -Wpedantic -Werror `
    -Iinclude `
    examples/example_no_tls.c `
    target/release/MMA.dll.lib `
    -o basic_server.exe

Copy-Item .\target\release\MMA.dll .\MMA.dll -Force
.\basic_server.exe
```

Для TLS вместо `example_no_tls.c` соберите `example_with_tls.c`, затем передайте пути к PEM-сертификату и приватному ключу:

```powershell
.\basic_server.exe server.crt server.key
```

Поле `MMA_TLSConfig.tls_handshake_timeout` задаётся в **миллисекундах** (`10000` — 10 секунд). DLL `MMA.dll` должна находиться рядом с исполняемым файлом либо в каталоге поиска Windows. На macOS используется `libMMA.dylib`; настройте `@rpath` или `DYLD_LIBRARY_PATH`.

### 8.2. Модель API и жизненный цикл сервера

Стандартная последовательность работы:

1. Вызвать `mma_server_config_default`, чтобы заполнить конфигурацию значениями по умолчанию.
2. Изменить нужные поля; для TLS установить `tls_config` в адрес корректной `MMA_TLSConfig`.
3. Вызвать `mma_server_create`.
4. Зарегистрировать route-обработчики, channel-обработчики и необязательные connect/disconnect hooks. Регистрация выполняется до запуска.
5. Вызвать `mma_server_start`. Функция запускает Tokio runtime в отдельном системном потоке и возвращает управление после готовности listener.
6. Только **после успешного `mma_server_start`** вызвать `mma_server_get_handle`, если callback-ам требуется отправлять сообщения в каналы или менять конфигурацию. `mma_server_local_port` также вызывается после запуска.
7. При завершении вызвать `mma_server_stop`, освободить handle через `mma_server_handle_destroy`, затем освободить сервер через `mma_server_destroy`.

Объект сервера рассчитан на один запуск. Не вызывайте `start` повторно, а также не вызывайте `stop`/`destroy` конкурентно для одного указателя. `mma_server_get_handle` вернёт ошибку, если сервер не запущен, runtime-поток завершился или внутренний handle недоступен.

### 8.3. Непрозрачный запрос и типы callback

Новые callback-и возвращают `void`:

```c
typedef void (*MMA_RouteCallback)(MMA_Request *request, void *user_data);
typedef void (*MMA_ChannelCallback)(
    MMA_Request *request,
    uint32_t connection_id,
    void *user_data
);
```

- Route-callback обрабатывает `Once`-запрос и должен завершить его вызовом `mma_request_complete`.
- Callback канала обрабатывает сообщение в канале или уведомление о жизненном цикле. Он получает `connection_id` и **не возвращает response**.
- Connect/disconnect hooks используют `MMA_ChannelCallback` и регистрируются глобально, без имени route.
- `user_data` передаётся callback-у без копирования. Если несколько callback-ов обращаются к общему изменяемому состоянию, синхронизируйте доступ и сохраняйте объект `user_data` живым до остановки сервера и завершения callback-ов.

Callback-и вызываются **синхронно на worker-потоках Tokio runtime**. Они не переносятся в C worker pool или `spawn_blocking`. Callback должен быстро вернуть управление и не выполнять блокирующий I/O или тяжёлую CPU-работу. Пока C-функция выполняется, она занимает runtime worker; `request_timeout` не может принудительно прервать выполняющуюся C-функцию. Для блокирующего I/O или CPU-задач удержите запрос через `retain`, поставьте задачу в собственный ограниченный worker pool и вернитесь из callback.

`MMA_Request` — непрозрачный тип. Получать данные можно только через функции API:

- `mma_request_get_info` заполняет `MMA_RequestInfo`: route, opcode, version, request ID и указатель/длину payload.
- `mma_request_option_count` возвращает число options.
- `mma_request_option_at` возвращает указатели и длины key/value для указанного индекса. Текущий wire decoder допускает повторяющиеся ключи; перебирайте options по индексу и не рассчитывайте на уникальность ключа.
- `mma_request_is_cancelled` позволяет проверить отмену удерживаемого route-запроса, например после timeout или shutdown.

Указатели на route и payload из `MMA_RequestInfo`, а также на key/value из `mma_request_option_at`, **заимствованы**. Они действительны, пока живёт соответствующая ссылка на запрос. Скопируйте данные, если они должны пережить эту ссылку.

### 8.4. Завершение route-запроса

Если обработка короткая, callback может прочитать запрос, создать response и завершить запрос до своего возврата:

```c
#include "mma.h"

static void echo_route(MMA_Request *request, void *user_data)
{
    (void)user_data;

    MMA_RequestInfo info;
    if (mma_request_get_info(request, &info) != MMA_OK) {
        return;
    }

    MMA_Response *response = mma_response_new(info.request_id);
    if (response == NULL) {
        return;
    }

    int32_t rc = mma_response_set_status(response, MMA_RESPONSE_OK);
    if (rc == MMA_OK) {
        rc = mma_response_set_payload(response, info.payload, info.payload_len);
    }
    if (rc != MMA_OK) {
        mma_response_destroy(response);
        return;
    }

    rc = mma_request_complete(request, response);
    if (rc == MMA_INVALID_ARGUMENT) {
        /* Владение не передано библиотеке. */
        mma_response_destroy(response);
    }
    /* При OK, ALREADY_COMPLETED и CANCELLED response уже потреблён. */
}
```

`mma_response_set_payload` и `mma_response_add_option` **копируют** переданные буферы. `mma_response_new` создаёт непрозрачный response, управляемый библиотекой; не выделяйте и не изменяйте его напрямую.

У `mma_request_complete` строгое правило владения:

- Для корректного route-запроса библиотека забирает `response` при `MMA_OK`, `MMA_REQUEST_ALREADY_COMPLETED` и `MMA_REQUEST_CANCELLED`.
- При `MMA_INVALID_ARGUMENT` владение остаётся у вызывающей стороны; созданный response необходимо уничтожить.
- Channel/lifecycle notification не ожидает response. Вызов `mma_request_complete` для него вернёт `MMA_INVALID_ARGUMENT`; response остаётся у вызывающей стороны.
- Route-запрос можно завершить только один раз. Повторный вызов возвращает `MMA_REQUEST_ALREADY_COMPLETED`.

Если создать response не удалось или callback вернулся без завершения запроса, запрос остаётся ожидающим до `request_timeout` или shutdown. При timeout сервер отправляет ответ `TIMEOUT`; сохранённый route-запрос может проверить отмену через `mma_request_is_cancelled`.

### 8.5. Отложенная работа: `retain` / `release`

Указатель на запрос действителен во время callback. Чтобы использовать его после возврата callback, получите дополнительную ссылку **внутри callback**:

```c
static void route_callback(MMA_Request *request, void *user_data)
{
    if (mma_request_retain(request) != MMA_OK) {
        return;
    }

    /* Псевдокод: submit_to_bounded_pool(worker, request, user_data). */
    if (!submit_to_bounded_pool(request, user_data)) {
        mma_request_release(request);
    }
}

static void worker(MMA_Request *request, void *user_data)
{
    /* Прочитать/скопировать данные, создать response и вызвать complete(). */
    /* ... */
    mma_request_release(request);
}
```

`retain` увеличивает внутренний счётчик ссылок; он **не копирует** запрос или payload. Каждый успешный `mma_request_retain` должен иметь ровно один соответствующий `mma_request_release`, в том числе при ошибке постановки в очередь и при отмене задачи. Освобождайте запрос только после того, как worker закончил работу с запросом и всеми заимствованными указателями. После освобождения своей последней ссылки нельзя обращаться к указателю.

Для удерживаемого route-запроса проверяйте `mma_request_is_cancelled` до дорогой работы или позднего завершения. Отмена не останавливает пользовательский worker автоматически: приложение должно само заметить отмену и прекратить свою работу.

### 8.6. Channel-сообщения и server handle

Callback-и канала и lifecycle hooks — это уведомления. Указатель `MMA_Request` предоставляет те же функции чтения, но `mma_request_complete` для них недопустим. Вызовите `retain`, если запрос нужно обработать после возврата callback.

Получайте handle только после успешного `mma_server_start`:

```c
MMA_ServerHandle *handle = NULL;
int32_t rc = mma_server_get_handle(server, &handle);
if (rc != MMA_OK || handle == NULL) {
    /* Не удалось получить handle. */
}
```

Для отправки сообщения используйте `mma_server_handle_send_to_channel`. Функция использует **неблокирующую отправку**. `conn_id == 0` рассылает сообщение всем channel-соединениям; ненулевой ID выбирает одно соединение. Библиотека копирует response через внутренний clone, поэтому после вызова вызывающая сторона может уничтожить свой response. `MMA_SERVER_ERROR` означает ошибку доставки (например, неизвестный ID, заполненная очередь на 256 сообщений или частично принятая рассылка). Это не гарантия доставки и не признак безопасного повтора.

`mma_server_handle_change_config` публикует проверенную новую конфигурацию. Активные соединения получают новую конфигурацию frame и `request_timeout`. Адрес/порт bind, `max_batch`, `max_connections`, `max_in_flight` и `shutdown_grace` динамически не меняются. TLS-настройки нельзя менять во время работы сервера. Обе функции handle можно вызывать из потоков приложения, пока сам handle остаётся действительным.

### 8.7. Функции и коды возврата

| Функция | Назначение |
|---|---|
| `mma_server_config_default`, `mma_server_create` | инициализация конфигурации и создание сервера |
| `mma_server_register_route` | регистрация Once route |
| `mma_server_register_channel_handler` | регистрация channel route |
| `mma_server_register_on_channel_connect_handler`, `...disconnect_handler` | регистрация lifecycle hooks |
| `mma_server_start`, `mma_server_local_port`, `mma_server_stop`, `mma_server_destroy` | жизненный цикл сервера и получение порта |
| `mma_request_retain`, `mma_request_release` | время жизни запроса после возврата callback |
| `mma_request_get_info`, `mma_request_option_count`, `mma_request_option_at`, `mma_request_is_cancelled` | чтение запроса и состояния отмены |
| `mma_request_complete` | завершение route-запроса с response |
| `mma_response_new`, `mma_response_set_status`, `mma_response_set_payload`, `mma_response_add_option`, `mma_response_destroy` | создание, заполнение и освобождение response |
| `mma_server_get_handle`, `mma_server_handle_destroy` | получение/освобождение клонируемого server handle |
| `mma_server_handle_send_to_channel`, `mma_server_handle_change_config` | push в канал и изменение конфигурации |

| Код | Имя | Значение |
|---:|---|---|
| 0 | `MMA_OK` | успех |
| −1 | `MMA_INVALID_ARGUMENT` | нулевой указатель или некорректный аргумент |
| −2 | `MMA_INVALID_CONFIG` | некорректная конфигурация |
| −3 | `MMA_SERVER_ERROR` | ошибка запуска/runtime/операции |
| −4 | `MMA_SERVER_ALREADY_STARTED` | сервер уже запущен |
| −5 | `MMA_REQUEST_ALREADY_COMPLETED` | route-запрос уже завершён (response потреблён) |
| −6 | `MMA_REQUEST_CANCELLED` | route-запрос отменён (response потреблён) |

Opcode ответов: `MMA_RESPONSE_OK`, `MMA_RESPONSE_BAD_REQUEST`, `MMA_RESPONSE_UNAUTHORIZED`, `MMA_RESPONSE_FORBIDDEN`, `MMA_RESPONSE_NOT_FOUND`, `MMA_RESPONSE_CONFLICT`, `MMA_RESPONSE_INTERNAL_ERROR`, `MMA_RESPONSE_CHANNEL_MESSAGE`, `MMA_RESPONSE_SHUTDOWN`, `MMA_RESPONSE_CHANNEL_CONNECTED` и `MMA_RESPONSE_TIMEOUT` (значения 1–11).

### 8.8. Конфигурация, строки и время жизни

`MMA_ServerConfig` содержит `const MMA_TLSConfig *tls_config`; `NULL` отключает TLS. Все поля timeout (`drain_timeout_ms`, `shutdown_grace_ms`, `request_timeout_ms` и `tls_handshake_timeout`) задаются в миллисекундах. При текущей раскладке `repr(C)` размер `MMA_ServerConfig` на x86-64 — 72 байта, `MMA_TLSConfig` — 24 байта. Всегда компилируйте C-код с `mma.h`, поставляемым с той native-библиотекой, с которой выполняется линковка.

Библиотека копирует `bind_ip`, пути к сертификату/ключу, имена зарегистрированных route, payload и буферы options во время соответствующего вызова API. Конфигурация и вложенная `MMA_TLSConfig` должны оставаться действительными на время `mma_server_create` или `mma_server_handle_change_config`; строки можно освободить после возврата вызова. `user_data` не копируется: храните его до остановки сервера и окончания callback-ов. Буферы для `mma_response_set_payload` и `mma_response_add_option` можно освобождать после возврата вызова.

- В POSIX C-хосте игнорируйте `SIGPIPE` до запуска сервера (`signal(SIGPIPE, SIG_IGN)`). Примеры демонстрируют обработку сигналов остановки. В Windows консольные события обрабатываются через Windows Console API.
- Пока используете `MMA_ServerHandle`, он должен оставаться действительным; уничтожайте его до уничтожения сервера. `mma_server_handle_destroy` не останавливает сервер.
- Не вызывайте `mma_server_stop` или `mma_server_destroy` из callback этого же сервера: `stop` ожидает завершения runtime-потока.
- Текущий C ABI несовместим с C-заголовком до async-переработки из предыдущего release snapshot. Перекомпилируйте приложение с новым заголовком и связывайте с соответствующей библиотекой; см. [§9](#9-совместимость-и-версионирование).

Полные реализации находятся в [`examples/example_no_tls.c`](examples/example_no_tls.c) и [`examples/example_with_tls.c`](examples/example_with_tls.c). В них показаны route `POW`, `ECHO`, `CHANGE`, channel-handler `SEND`, lifecycle callbacks, владение response, работа с server handle и корректное завершение сервера.

---

## 9. Совместимость и версионирование

Три версии независимы:

| Что | Сейчас | Правило |
|---|---|---|
| Wire-протокол | `MMA_VERSION = 1` | повышается при несовместимых изменениях кадра или семантики |
| Rust crate | `0.1.0` в `Cargo.toml` (git-теги доходят до `v0.1.4`) | до 1.0 breaking-изменения возможны в любом релизе; изменились пути модулей и API push (см. [CHANGELOG.ru.md](CHANGELOG.ru.md)) |
| C ABI / `mma.h` | без версии | Breaking change относительно предыдущего release snapshot: `MMA_Request` и `MMA_Response` стали непрозрачными handle; сигнатуры callback изменены на `void`, а route-response завершается через `mma_request_complete`. Пересоберите все C-приложения с актуальным `mma.h`; запроса ABI-версии в рантайме нет |

Два участника совместимы по wire, только если совпадают `MMA_VERSION` **и** весь `FramerConfig`. Замечание: правило зарезервированного route для `ChannelConnect` ([§5.5](#55-зарезервированные-маршруты-и-хуки-жизненного-цикла-соединения)) меняет семантику запроса, не меняя раскладку кадра и `MMA_VERSION`; см. [CHANGELOG.ru.md](CHANGELOG.ru.md). Новые opcode или обязательное поле требуют новой `MMA_VERSION`; старый сервер закрывает соединение при неизвестном opcode запроса.

---

## 10. Безопасность

- **TLS необязателен и только на стороне сервера** (см. [§7.2](#72-конфигурация)); нет клиентской аутентификации, аутентификации на уровне приложения и защиты от replay. Без TLS всё, включая раскладку, читается в сети. Конфигурация раскладки — обфускация, а не механизм безопасности. Если сертификат или ключ не удаётся загрузить, инициализация сервера завершается ошибкой; незашифрованный TCP вместо TLS не запускается.
- Репозиторий не содержит сертификатов и приватных ключей. При запуске TLS-примера предоставьте сертификат и соответствующий приватный ключ. Для локального тестирования используйте отдельные тестовые credentials; не добавляйте production-ключи в репозиторий и не распространяйте их.
- Считайте все поля от клиента недоверенными. UTF-8 проверяется только у `route`; payload и options — сырые байты.

| Лимит | Значение |
|---|---|
| Тело кадра | `max_message_length` (по умолчанию 10 000 000), проверяется до аллокации |
| Route | ≤ 255 байт |
| Options | ≤ 255 на кадр; key и value по 1–65535 байт |
| Соединения | `max_connections` |
| Запросы в полёте | `max_in_flight` глобально; лимита на соединение нет |
| Очередь push-сообщений | 256 сообщений на channel-соединение |
| Время обработчика | `request_timeout` для async-обработки; выполняющийся синхронный C-callback нельзя прервать |

Пока не покрыто: idle/read/write-таймауты (атаки медленными клиентами), глобальный лимит байт в полёте и защита от клиентов, не читающих ответы (они же останавливают `send_to_channel(0, ..)`, см. [§5.4](#54-режим-channel)).


---

## 11. Структура репозитория

| Путь | Содержимое |
|---|---|
| `src/protocol/framer.rs` | энкодер/декодер кадров (приватный) |
| `src/network/network.rs` | цикл соединения, машина состояний (приватный) |
| `src/config/config.rs` | `Config`, `FramerConfig`, `BigBytesConfig`, `MMA_VERSION` |
| `src/core/` | `commands` (Request/Response, `SocketStatus`), `router` (маршруты, обработчики, хуки жизненного цикла), `server`, `handle` (`ServerHandle`), `channel_hub` и `tls` (приватные) |
| `src/c_api.rs` | экспорты C ABI (непрозрачные handle, refcount запросов и асинхронное завершение route) |
| `include/mma.h` | C-заголовок |
| `.gitattributes` | нормализует окончания строк текстовых файлов к LF между операционными системами |
| `examples/example_no_tls.rs`, `examples/example_with_tls.rs` | примеры сервера на Rust без TLS и с TLS; оба показывают обработчики маршрутов и каналов, а также хуки жизненного цикла соединения |
| `examples/example_no_tls.c`, `examples/example_with_tls.c` | примеры сервера через C ABI без TLS и с TLS; TLS-пример принимает пути к сертификату и приватному ключу |
| `mma_gui_client.py` | тестовый клиент на Tkinter (стандартная библиотека Python): настраиваемая раскладка, все три opcode, самотест кодека |
| `.github/workflows/native-release.yml` | CI при push, pull request и ручном запуске: тесты Rust, сборка release-библиотеки и примеров, компиляция обоих C-примеров на Linux (Clang) и Windows (MSVC), проверка публичного заголовка из C++. На тегах `v*` собирает четыре native-пакета (Windows x86-64, Linux x86-64, macOS x86-64 и macOS arm64), проверяет SHA-256 и публикует GitHub Release |
| `CHANGELOG.md`, `CHANGELOG.ru.md` | история изменений на английском и русском с влиянием на wire / Rust API / C ABI |
| `tests/` | тесты конфигурации (тесты кодека сейчас отключены) |
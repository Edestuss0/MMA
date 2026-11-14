#define _POSIX_C_SOURCE 200809L

#include "mma.h"

#include <errno.h>
#include <inttypes.h>
#include <limits.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
    #include <windows.h>
#else
    #include <unistd.h>
#endif

#define MMA_EXAMPLE_PORT 8080u
#define MMA_EXAMPLE_MAX_MESSAGE (10u * 1000u * 1000u)

typedef struct AppContext {
    MMA_ServerHandle *handle;
    uint16_t port;
} AppContext;

static volatile sig_atomic_t g_stop_requested = 0;

#ifdef _WIN32
static BOOL WINAPI handle_console_signal(DWORD signal_type)
{
    switch (signal_type) {
        case CTRL_C_EVENT:
        case CTRL_BREAK_EVENT:
        case CTRL_CLOSE_EVENT:
        case CTRL_SHUTDOWN_EVENT:
            g_stop_requested = 1;
            return TRUE;
        default:
            return FALSE;
    }
}
#else
static void handle_signal(int signal_number)
{
    (void)signal_number;
    g_stop_requested = 1;
}
#endif

static int install_shutdown_handlers(void)
{
#ifdef _WIN32
    if (!SetConsoleCtrlHandler(handle_console_signal, TRUE)) {
        fprintf(stderr, "SetConsoleCtrlHandler failed: %lu\n",
                (unsigned long)GetLastError());
        return 0;
    }
#else
    struct sigaction action;
    memset(&action, 0, sizeof(action));
    action.sa_handler = handle_signal;
    if (sigemptyset(&action.sa_mask) != 0 ||
        sigaction(SIGINT, &action, NULL) != 0 ||
        sigaction(SIGTERM, &action, NULL) != 0) {
        perror("install signal handlers");
        return 0;
    }
#endif
    return 1;
}

static void remove_shutdown_handlers(void)
{
#ifdef _WIN32
    (void)SetConsoleCtrlHandler(handle_console_signal, FALSE);
#endif
}

static int fill_config(MMA_ServerConfig *config,
                       uint16_t port,
                       const MMA_TLSConfig *tls_config,
                       int changed_framer)
{
    int32_t rc = mma_server_config_default(config);
    if (rc != MMA_OK) {
        fprintf(stderr, "mma_server_config_default: %d\n", (int)rc);
        return 0;
    }

    config->bind_ip = "0.0.0.0";
    config->port = port;
    config->max_batch = 65535;
    config->max_in_flight = 65535;
    config->max_connections = 128000;
    config->drain_timeout_ms = 2000;
    config->shutdown_grace_ms = 10000;
    config->request_timeout_ms = 10000;
    config->tls_config = tls_config;

    config->framer.max_message_length = MMA_EXAMPLE_MAX_MESSAGE;
    config->framer.opcode_pos = 1;
    config->framer.version_pos = 4;
    config->framer.route_len_pos = 6;
    config->framer.route_order = 3;
    config->framer.payload_order = 1;
    config->framer.options_order = 2;
    config->framer.options_count_pos = 7;
    config->framer.options_key_first = changed_framer ? 0 : 1;
    config->framer.header_length = changed_framer ? 35 : 29;

    config->framer.big_bytes_config.options_len_order = changed_framer ? 2 : 1;
    config->framer.big_bytes_config.payload_len_order = changed_framer ? 1 : 2;
    config->framer.big_bytes_config.req_id_order = 3;

    return 1;
}

static int complete_response(MMA_Request *request,
                             uint32_t request_id,
                             uint8_t status,
                             const uint8_t *payload,
                             size_t payload_len)
{
    MMA_Response *response = mma_response_new(request_id);
    if (response == NULL) {
        fprintf(stderr, "mma_response_new failed\n");
        return 0;
    }

    int32_t rc = mma_response_set_status(response, status);
    if (rc != MMA_OK) {
        fprintf(stderr, "mma_response_set_status: %d\n", (int)rc);
        mma_response_destroy(response);
        return 0;
    }

    if (payload == NULL && payload_len != 0) {
        fprintf(stderr, "complete_response: NULL payload with non-zero length\n");
        mma_response_destroy(response);
        return 0;
    }

    rc = mma_response_set_payload(
        response,
        payload != NULL ? payload : (const uint8_t *)"",
        payload_len
    );
    if (rc != MMA_OK) {
        fprintf(stderr, "mma_response_set_payload: %d\n", (int)rc);
        mma_response_destroy(response);
        return 0;
    }

    /* Ownership transfers on every result except MMA_INVALID_ARGUMENT. */
    rc = mma_request_complete(request, response);
    if (rc == MMA_INVALID_ARGUMENT) {
        mma_response_destroy(response);
        fprintf(stderr, "mma_request_complete: invalid request type\n");
        return 0;
    }
    if (rc == MMA_REQUEST_ALREADY_COMPLETED || rc == MMA_REQUEST_CANCELLED) {
        return 0;
    }
    if (rc != MMA_OK) {
        fprintf(stderr, "mma_request_complete: %d\n", (int)rc);
        return 0;
    }

    return 1;
}

static int get_request_info(MMA_Request *request, MMA_RequestInfo *info)
{
    memset(info, 0, sizeof(*info));
    int32_t rc = mma_request_get_info(request, info);
    if (rc != MMA_OK) {
        fprintf(stderr, "mma_request_get_info: %d\n", (int)rc);
        return 0;
    }
    return 1;
}

static int is_ascii_space(uint8_t c)
{
    return c == (uint8_t)' ' || c == (uint8_t)'\t' ||
           c == (uint8_t)'\n' || c == (uint8_t)'\r' ||
           c == (uint8_t)'\f' || c == (uint8_t)'\v';
}

/* Parses the same ordinary signed decimal input expected by the Rust example. */
static int parse_i32(const uint8_t *data, size_t len, int32_t *output)
{
    if (data == NULL || output == NULL) {
        return 0;
    }

    size_t i = 0;
    while (i < len && is_ascii_space(data[i])) {
        ++i;
    }

    int negative = 0;
    if (i < len && (data[i] == (uint8_t)'+' || data[i] == (uint8_t)'-')) {
        negative = data[i] == (uint8_t)'-';
        ++i;
    }

    if (i >= len || data[i] < (uint8_t)'0' || data[i] > (uint8_t)'9') {
        return 0;
    }

    const uint64_t limit = negative ? UINT64_C(2147483648) : UINT64_C(2147483647);
    uint64_t value = 0;
    while (i < len && data[i] >= (uint8_t)'0' && data[i] <= (uint8_t)'9') {
        uint64_t digit = (uint64_t)(data[i] - (uint8_t)'0');
        if (value > (limit - digit) / 10u) {
            return 0;
        }
        value = value * 10u + digit;
        ++i;
    }

    while (i < len && is_ascii_space(data[i])) {
        ++i;
    }
    if (i != len) {
        return 0;
    }

    int64_t signed_value = negative ? -(int64_t)value : (int64_t)value;
    *output = (int32_t)signed_value;
    return 1;
}

static int parse_u32_option(const uint8_t *data, size_t len, uint32_t *output)
{
    if (data == NULL || output == NULL || len == 0) {
        return 0;
    }

    size_t i = 0;
    if (data[i] == (uint8_t)'+') {
        ++i;
    }
    if (i == len) {
        return 0;
    }

    uint64_t value = 0;
    for (; i < len; ++i) {
        if (data[i] < (uint8_t)'0' || data[i] > (uint8_t)'9') {
            return 0;
        }
        uint64_t digit = (uint64_t)(data[i] - (uint8_t)'0');
        if (value > (UINT32_MAX - digit) / 10u) {
            return 0;
        }
        value = value * 10u + digit;
    }

    *output = (uint32_t)value;
    return 1;
}

/* Returns 1 when the option exists, 0 when absent, and -1 on ABI/parse error. */
static int get_pow_option(MMA_Request *request, uint32_t *power)
{
    size_t count = 0;
    int32_t rc = mma_request_option_count(request, &count);
    if (rc != MMA_OK) {
        fprintf(stderr, "mma_request_option_count: %d\n", (int)rc);
        return -1;
    }

    for (size_t i = 0; i < count; ++i) {
        const uint8_t *key = NULL;
        const uint8_t *value = NULL;
        size_t key_len = 0;
        size_t value_len = 0;

        rc = mma_request_option_at(
            request, i, &key, &key_len, &value, &value_len
        );
        if (rc != MMA_OK) {
            fprintf(stderr, "mma_request_option_at: %d\n", (int)rc);
            return -1;
        }

        if (key != NULL && key_len == 3 && memcmp(key, "pow", 3) == 0) {
            return parse_u32_option(value, value_len, power) ? 1 : -1;
        }
    }

    return 0;
}

static int checked_mul_i32(int32_t left, int32_t right, int32_t *output)
{
    int64_t product = (int64_t)left * (int64_t)right;
    if (product < INT32_MIN || product > INT32_MAX) {
        return 0;
    }
    *output = (int32_t)product;
    return 1;
}

static int checked_pow_i32(int32_t base, uint32_t exponent, int32_t *output)
{
    int32_t result = 1;
    int32_t factor = base;

    while (exponent != 0) {
        if ((exponent & 1u) != 0 && !checked_mul_i32(result, factor, &result)) {
            return 0;
        }
        exponent >>= 1u;
        if (exponent != 0 && !checked_mul_i32(factor, factor, &factor)) {
            return 0;
        }
    }

    *output = result;
    return 1;
}

static void echo_route(MMA_Request *request, void *user_data)
{
    (void)user_data;
    MMA_RequestInfo info;
    if (!get_request_info(request, &info)) {
        return;
    }

    (void)complete_response(
        request, info.request_id, MMA_RESPONSE_OK,
        info.payload, info.payload_len
    );
}

static void pow_route(MMA_Request *request, void *user_data)
{
    (void)user_data;
    MMA_RequestInfo info;
    if (!get_request_info(request, &info)) {
        return;
    }

    int32_t number = 0;
    if (!parse_i32(info.payload, info.payload_len, &number)) {
        static const uint8_t error[] = "Invalid payload";
        (void)complete_response(
            request, info.request_id, MMA_RESPONSE_BAD_REQUEST,
            error, sizeof(error) - 1u
        );
        return;
    }

    uint32_t exponent = 2;
    int option_result = get_pow_option(request, &exponent);
    if (option_result < 0) {
        static const uint8_t error[] = "Invalid pow option";
        (void)complete_response(
            request, info.request_id, MMA_RESPONSE_BAD_REQUEST,
            error, sizeof(error) - 1u
        );
        return;
    }

    int32_t result = 0;
    if (!checked_pow_i32(number, exponent, &result)) {
        static const uint8_t error[] = "Result overflows int32";
        (void)complete_response(
            request, info.request_id, MMA_RESPONSE_BAD_REQUEST,
            error, sizeof(error) - 1u
        );
        return;
    }

    char body[128];
    int length = snprintf(
        body, sizeof(body), "%" PRId32 "^%" PRIu32 " = %" PRId32,
        number, exponent, result
    );
    if (length < 0 || (size_t)length >= sizeof(body)) {
        static const uint8_t error[] = "Response formatting failed";
        (void)complete_response(
            request, info.request_id, MMA_RESPONSE_INTERNAL_ERROR,
            error, sizeof(error) - 1u
        );
        return;
    }

    (void)complete_response(
        request, info.request_id, MMA_RESPONSE_OK,
        (const uint8_t *)body, (size_t)length
    );
}

/* Response::new(0) in the Rust example: request id zero, default status. */
static void send_channel_payload(AppContext *context,
                                 uint32_t connection_id,
                                 const uint8_t *payload,
                                 size_t payload_len)
{
    if (context == NULL || context->handle == NULL) {
        return;
    }

    MMA_Response *message = mma_response_new(0);
    if (message == NULL) {
        fprintf(stderr, "channel: mma_response_new failed\n");
        return;
    }

    int32_t rc = mma_response_set_payload(
        message,
        payload != NULL ? payload : (const uint8_t *)"",
        payload_len
    );
    if (rc != MMA_OK) {
        fprintf(stderr, "channel: mma_response_set_payload: %d\n", (int)rc);
        mma_response_destroy(message);
        return;
    }

    rc = mma_server_handle_send_to_channel(context->handle, connection_id, message);
    if (rc != MMA_OK) {
        fprintf(stderr, "mma_server_handle_send_to_channel: %d\n", (int)rc);
    }

    /* The handle API takes a borrowed const response; it does not consume it. */
    mma_response_destroy(message);
}

static int is_valid_utf8(const uint8_t *data, size_t len)
{
    size_t i = 0;
    if (data == NULL && len != 0) {
        return 0;
    }

    while (i < len) {
        uint8_t c = data[i];
        if (c <= 0x7Fu) {
            ++i;
            continue;
        }

        if (c >= 0xC2u && c <= 0xDFu) {
            if (i + 1u >= len || (data[i + 1u] & 0xC0u) != 0x80u) return 0;
            i += 2u;
            continue;
        }

        if (c >= 0xE0u && c <= 0xEFu) {
            if (i + 2u >= len) return 0;
            uint8_t c1 = data[i + 1u];
            uint8_t c2 = data[i + 2u];
            if ((c2 & 0xC0u) != 0x80u) return 0;
            if (c == 0xE0u) {
                if (c1 < 0xA0u || c1 > 0xBFu) return 0;
            } else if (c == 0xEDu) {
                if (c1 < 0x80u || c1 > 0x9Fu) return 0;
            } else if ((c1 & 0xC0u) != 0x80u) {
                return 0;
            }
            i += 3u;
            continue;
        }

        if (c >= 0xF0u && c <= 0xF4u) {
            if (i + 3u >= len) return 0;
            uint8_t c1 = data[i + 1u];
            uint8_t c2 = data[i + 2u];
            uint8_t c3 = data[i + 3u];
            if ((c2 & 0xC0u) != 0x80u || (c3 & 0xC0u) != 0x80u) return 0;
            if (c == 0xF0u) {
                if (c1 < 0x90u || c1 > 0xBFu) return 0;
            } else if (c == 0xF4u) {
                if (c1 < 0x80u || c1 > 0x8Fu) return 0;
            } else if ((c1 & 0xC0u) != 0x80u) {
                return 0;
            }
            i += 4u;
            continue;
        }

        return 0;
    }

    return 1;
}

static void send_lifecycle_message(AppContext *context,
                                   const char *event_name,
                                   const uint8_t *payload,
                                   size_t payload_len,
                                   uint32_t connection_id)
{
    static const uint8_t invalid_data[] = "Invalid data";
    if (!is_valid_utf8(payload, payload_len)) {
        payload = invalid_data;
        payload_len = sizeof(invalid_data) - 1u;
    }

    char suffix[64];
    int suffix_len = snprintf(suffix, sizeof(suffix), ", id: %u", (unsigned)connection_id);
    if (suffix_len < 0 || (size_t)suffix_len >= sizeof(suffix)) {
        return;
    }

    static const char prefix_mid[] = ": payload: ";
    size_t event_len = strlen(event_name);
    size_t mid_len = sizeof(prefix_mid) - 1u;
    size_t suffix_size = (size_t)suffix_len;

    if (event_len > SIZE_MAX - mid_len ||
        event_len + mid_len > SIZE_MAX - payload_len ||
        event_len + mid_len + payload_len > SIZE_MAX - suffix_size) {
        fprintf(stderr, "lifecycle message is too large\n");
        return;
    }

    size_t total_len = event_len + mid_len + payload_len + suffix_size;
    uint8_t *message = (uint8_t *)malloc(total_len == 0 ? 1u : total_len);
    if (message == NULL) {
        fprintf(stderr, "lifecycle message allocation failed\n");
        return;
    }

    size_t offset = 0;
    memcpy(message + offset, event_name, event_len);
    offset += event_len;
    memcpy(message + offset, prefix_mid, mid_len);
    offset += mid_len;
    if (payload_len != 0) {
        memcpy(message + offset, payload, payload_len);
        offset += payload_len;
    }
    memcpy(message + offset, suffix, suffix_size);
    offset += suffix_size;

    send_channel_payload(context, 0, message, offset);
    free(message);
}

static void send_channel(MMA_Request *request,
                         uint32_t connection_id,
                         void *user_data)
{
    AppContext *context = (AppContext *)user_data;
    MMA_RequestInfo info;
    (void)connection_id; /* The Rust example intentionally sends to channel 0. */

    if (!get_request_info(request, &info)) {
        return;
    }

    send_channel_payload(context, 0, info.payload, info.payload_len);
}

static void channel_connected(MMA_Request *request,
                              uint32_t connection_id,
                              void *user_data)
{
    AppContext *context = (AppContext *)user_data;
    MMA_RequestInfo info;
    if (!get_request_info(request, &info)) {
        return;
    }

    send_lifecycle_message(
        context, "Connect", info.payload, info.payload_len, connection_id
    );
}

static void channel_disconnected(MMA_Request *request,
                                 uint32_t connection_id,
                                 void *user_data)
{
    AppContext *context = (AppContext *)user_data;
    MMA_RequestInfo info;
    if (!get_request_info(request, &info)) {
        return;
    }

    send_lifecycle_message(
        context, "Disconnect", info.payload, info.payload_len, connection_id
    );
}

static void change_route(MMA_Request *request, void *user_data)
{
    AppContext *context = (AppContext *)user_data;
    MMA_RequestInfo info;
    if (!get_request_info(request, &info)) {
        return;
    }

    MMA_ServerConfig changed_config;
    if (!fill_config(&changed_config, context->port, NULL, 1)) {
        static const uint8_t error[] = "Failed to construct config";
        (void)complete_response(
            request, info.request_id, MMA_RESPONSE_INTERNAL_ERROR,
            error, sizeof(error) - 1u
        );
        return;
    }

    int32_t rc = mma_server_handle_change_config(context->handle, &changed_config);
    if (rc != MMA_OK) {
        fprintf(stderr, "mma_server_handle_change_config: %d\n", (int)rc);
        static const uint8_t error[] = "Configuration change failed";
        (void)complete_response(
            request, info.request_id, MMA_RESPONSE_INTERNAL_ERROR,
            error, sizeof(error) - 1u
        );
        return;
    }

    /* The Rust CHANGE route returns its original, otherwise-empty response. */
    (void)complete_response(request, info.request_id, MMA_RESPONSE_OK, NULL, 0);
}

static int register_route(MMA_Server *server,
                          const char *name,
                          MMA_RouteCallback callback,
                          void *user_data)
{
    int32_t rc = mma_server_register_route(
        server,
        (const uint8_t *)name,
        strlen(name),
        callback,
        user_data
    );
    if (rc != MMA_OK) {
        fprintf(stderr, "register route %s: %d\n", name, (int)rc);
        return 0;
    }
    return 1;
}

static int run_server(uint16_t port, const MMA_TLSConfig *tls_config)
{
    MMA_ServerConfig config;
    if (!fill_config(&config, port, tls_config, 0)) {
        return 1;
    }

    MMA_Server *server = NULL;
    int32_t rc = mma_server_create(&config, &server);
    if (rc != MMA_OK || server == NULL) {
        fprintf(stderr, "mma_server_create: %d\n", (int)rc);
        return 1;
    }

    int exit_code = 1;
    int started = 0;
    MMA_ServerHandle *handle = NULL;
    AppContext context;
    context.handle = NULL;
    context.port = port;

    /* The handle becomes available only after mma_server_start succeeds. */

    if (!register_route(server, "POW", pow_route, NULL) ||
        !register_route(server, "ECHO", echo_route, NULL) ||
        !register_route(server, "CHANGE", change_route, &context)) {
        goto cleanup;
    }

    rc = mma_server_register_channel_handler(
        server, (const uint8_t *)"SEND", sizeof("SEND") - 1u,
        send_channel, &context
    );
    if (rc != MMA_OK) {
        fprintf(stderr, "register channel handler SEND: %d\n", (int)rc);
        goto cleanup;
    }

    rc = mma_server_register_on_channel_connect_handler(
        server, channel_connected, &context
    );
    if (rc != MMA_OK) {
        fprintf(stderr, "register connect handler: %d\n", (int)rc);
        goto cleanup;
    }

    rc = mma_server_register_on_channel_disconnect_handler(
        server, channel_disconnected, &context
    );
    if (rc != MMA_OK) {
        fprintf(stderr, "register disconnect handler: %d\n", (int)rc);
        goto cleanup;
    }

    rc = mma_server_start(server);
    if (rc != MMA_OK) {
        fprintf(stderr, "mma_server_start: %d\n", (int)rc);
        goto cleanup;
    }
    started = 1;

    rc = mma_server_get_handle(server, &handle);
    if (rc != MMA_OK || handle == NULL) {
        fprintf(stderr, "mma_server_get_handle: %d\n", (int)rc);
        goto cleanup;
    }
    context.handle = handle;

    uint16_t actual_port = port;
    rc = mma_server_local_port(server, &actual_port);
    if (rc != MMA_OK) {
        fprintf(stderr, "mma_server_local_port: %d\n", (int)rc);
        goto cleanup;
    }

    g_stop_requested = 0;
    if (!install_shutdown_handlers()) {
        goto cleanup;
    }

    printf("MMA server listening on %s:%u\n", config.bind_ip, (unsigned)actual_port);
    printf("Routes: POW, ECHO, CHANGE; channel handler: SEND\n");
    printf("TLS: %s\n", tls_config != NULL ? "enabled" : "disabled");
    printf("Press Ctrl+C to stop.\n");
    fflush(stdout);

    while (!g_stop_requested) {
#ifdef _WIN32
        Sleep(100);
#else
        (void)sleep(1);
#endif
    }

    exit_code = 0;

cleanup:
    remove_shutdown_handlers();

    if (started) {
        int32_t stop_rc = mma_server_stop(server);
        if (stop_rc != MMA_OK) {
            fprintf(stderr, "mma_server_stop: %d\n", (int)stop_rc);
            exit_code = 1;
        }
    }

    if (handle != NULL) {
        mma_server_handle_destroy(handle);
    }
    mma_server_destroy(server);
    return exit_code;
}

static int parse_port(const char *text, uint16_t *port_out)
{
    if (text == NULL || port_out == NULL || *text == '\0') {
        return 0;
    }

    errno = 0;
    char *end = NULL;
    unsigned long value = strtoul(text, &end, 10);
    if (errno != 0 || end == text || *end != '\0' || value == 0 || value > UINT16_MAX) {
        return 0;
    }

    *port_out = (uint16_t)value;
    return 1;
}

int main(int argc, char **argv)
{
    uint16_t port = (uint16_t)MMA_EXAMPLE_PORT;

    if (argc > 2) {
        fprintf(stderr, "Usage: %s [port]\n", argv[0]);
        return 2;
    }

    if (argc == 2 && !parse_port(argv[1], &port)) {
        fprintf(stderr, "Invalid port: %s\n", argv[1]);
        return 2;
    }

    return run_server(port, NULL);
}

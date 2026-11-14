#ifndef MMA_H
#define MMA_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Status codes returned by the library. */
enum {
    MMA_OK = 0,
    MMA_INVALID_ARGUMENT = -1,
    MMA_INVALID_CONFIG = -2,
    MMA_SERVER_ERROR = -3,
    MMA_SERVER_ALREADY_STARTED = -4,
    MMA_REQUEST_ALREADY_COMPLETED = -5,
    MMA_REQUEST_CANCELLED = -6
};

/* ResponseOpcode values from src/core/commands.rs. */
enum {
    MMA_RESPONSE_OK = 1,
    MMA_RESPONSE_BAD_REQUEST = 2,
    MMA_RESPONSE_UNAUTHORIZED = 3,
    MMA_RESPONSE_FORBIDDEN = 4,
    MMA_RESPONSE_NOT_FOUND = 5,
    MMA_RESPONSE_CONFLICT = 6,
    MMA_RESPONSE_INTERNAL_ERROR = 7,
    MMA_RESPONSE_CHANNEL_MESSAGE = 8,
    MMA_RESPONSE_SHUTDOWN = 9,
    MMA_RESPONSE_CHANNEL_CONNECTED = 10,
    MMA_RESPONSE_TIMEOUT = 11
};

/* Opaque library-owned objects. Never allocate these in C. */
typedef struct MMA_Server MMA_Server;
typedef struct MMA_ServerHandle MMA_ServerHandle;
typedef struct MMA_Request MMA_Request;
typedef struct MMA_Response MMA_Response;

typedef struct MMA_BigBytesConfig {
    uint8_t options_len_order;
    uint8_t payload_len_order;
    uint8_t req_id_order;
} MMA_BigBytesConfig;

typedef struct MMA_FramerConfig {
    uint32_t max_message_length;
    uint8_t opcode_pos;
    uint8_t version_pos;
    uint8_t route_len_pos;
    uint8_t route_order;
    uint8_t payload_order;
    uint8_t options_order;
    uint8_t options_count_pos;
    uint8_t options_key_first;
    uint8_t header_length;
    MMA_BigBytesConfig big_bytes_config;
} MMA_FramerConfig;

typedef struct MMA_TLSConfig {
    const char *tls_certificate_path;
    const char *tls_key_path;
    uint64_t tls_handshake_timeout; /* milliseconds */
} MMA_TLSConfig;

/* All *_timeout_ms fields are expressed in milliseconds. */
typedef struct MMA_ServerConfig {
    const char *bind_ip;
    uint16_t port;
    uint32_t max_batch;
    uint32_t max_in_flight;
    MMA_FramerConfig framer;
    uint32_t max_connections;
    uint64_t drain_timeout_ms;
    uint64_t shutdown_grace_ms;
    uint64_t request_timeout_ms;
    const MMA_TLSConfig *tls_config;
} MMA_ServerConfig;

/* route/payload are borrowed slices backed by the retained request. */
typedef struct MMA_RequestInfo {
    const uint8_t *route;
    size_t route_len;
    uint8_t opcode;
    uint8_t version;
    uint32_t request_id;
    const uint8_t *payload;
    size_t payload_len;
} MMA_RequestInfo;

/*
 * Callbacks execute synchronously on Tokio runtime worker threads and must
 * return quickly. To defer work, call mma_request_retain() inside the callback,
 * move the request to an application-owned executor, then release it exactly
 * once after the deferred work is finished.
 *
 * Route callbacks must complete the request with mma_request_complete().
 * Channel and lifecycle callbacks are notifications and do not return a
 * response.
 */
typedef void (*MMA_RouteCallback)(
    MMA_Request *request,
    void *user_data
);

typedef void (*MMA_ChannelCallback)(
    MMA_Request *request,
    uint32_t connection_id,
    void *user_data
);

/* Server configuration and lifecycle. */
int32_t mma_server_config_default(MMA_ServerConfig *output);
int32_t mma_server_create(const MMA_ServerConfig *config, MMA_Server **output);
int32_t mma_server_register_route(
    MMA_Server *server, const uint8_t *route, size_t route_len,
    MMA_RouteCallback callback, void *user_data);
int32_t mma_server_register_channel_handler(
    MMA_Server *server, const uint8_t *route, size_t route_len,
    MMA_ChannelCallback callback, void *user_data);
int32_t mma_server_register_on_channel_connect_handler(
    MMA_Server *server, MMA_ChannelCallback callback, void *user_data);
int32_t mma_server_register_on_channel_disconnect_handler(
    MMA_Server *server, MMA_ChannelCallback callback, void *user_data);
int32_t mma_server_start(MMA_Server *server);
int32_t mma_server_local_port(const MMA_Server *server, uint16_t *port);
int32_t mma_server_stop(MMA_Server *server);
void mma_server_destroy(MMA_Server *server);

/* Request lifetime and inspection. Retain inside callback before deferring. */
int32_t mma_request_retain(MMA_Request *request);
void mma_request_release(MMA_Request *request);
int32_t mma_request_get_info(
    const MMA_Request *request, MMA_RequestInfo *output);
int32_t mma_request_option_count(
    const MMA_Request *request, size_t *output);
int32_t mma_request_option_at(
    const MMA_Request *request, size_t index,
    const uint8_t **key, size_t *key_len,
    const uint8_t **value, size_t *value_len);
int32_t mma_request_is_cancelled(
    const MMA_Request *request, uint8_t *output);

/*
 * For route requests, completion consumes response ownership. If the
 * request was already completed or cancelled, the response is still
 * consumed. For non-response notifications this returns INVALID_ARGUMENT;
 * in that case the caller retains ownership of response.
 */
int32_t mma_request_complete(
    MMA_Request *request, MMA_Response *response);

/* Response construction. Payload and option buffers are copied. */
MMA_Response *mma_response_new(uint32_t request_id);
int32_t mma_response_set_status(MMA_Response *response, uint8_t opcode);
int32_t mma_response_set_payload(
    MMA_Response *response, const uint8_t *data, size_t data_len);
int32_t mma_response_add_option(
    MMA_Response *response,
    const uint8_t *key, size_t key_len,
    const uint8_t *value, size_t value_len);
void mma_response_destroy(MMA_Response *response);

/* Server handle operations. */
/* Obtain a handle only after mma_server_start() has returned MMA_OK. */
int32_t mma_server_get_handle(
    const MMA_Server *server, MMA_ServerHandle **output);
void mma_server_handle_destroy(MMA_ServerHandle *handle);
/* Non-blocking send; message is borrowed and is not consumed. conn_id == 0 broadcasts. */
int32_t mma_server_handle_send_to_channel(
    const MMA_ServerHandle *handle,
    uint32_t conn_id,
    const MMA_Response *message);
int32_t mma_server_handle_change_config(
    const MMA_ServerHandle *handle,
    const MMA_ServerConfig *config);

#ifdef __cplusplus
}
#endif

#endif /* MMA_H */

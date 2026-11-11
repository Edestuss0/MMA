#define _POSIX_C_SOURCE 200809L

#include "mma.h"

#include <errno.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static volatile sig_atomic_t g_stop_requested = 0;

static void handle_signal(int signal_number)
{
    (void)signal_number;
    g_stop_requested = 1;
}

static int register_route(
    MMA_Server *server,
    const char *name,
    MMA_AsyncRouteCallback callback)
{
    return mma_server_register_async_route(
        server,
        (const uint8_t *)name,
        strlen(name),
        callback,
        NULL
    );
}

/*
 * Route callbacks are short and non-blocking. Request data is borrowed
 * for the duration of the callback, so response_set_payload() copies it
 * before we complete the request.
 */
static void complete_request(
    MMA_AsyncRequest *request,
    uint32_t request_id,
    const uint8_t *payload,
    size_t payload_len)
{
    MMA_Response *response = mma_response_new(request_id);
    if (response == NULL) {
        fprintf(stderr, "mma_response_new failed\n");
        return; /* The request will finish through timeout/cancellation. */
    }

    int32_t rc = mma_response_set_status(response, MMA_RESPONSE_OK);
    if (rc != MMA_OK) {
        fprintf(stderr, "mma_response_set_status: %d\n", (int)rc);
        mma_response_destroy(response);
        return;
    }

    rc = mma_response_set_payload(response, payload, payload_len);
    if (rc != MMA_OK) {
        fprintf(stderr, "mma_response_set_payload: %d\n", (int)rc);
        mma_response_destroy(response);
        return;
    }

    /*
     * For a valid route request, complete() consumes response:
     *   MMA_OK                       -> sent to the waiting connection
     *   MMA_REQUEST_ALREADY_COMPLETED -> consumed, do not destroy
     *   MMA_REQUEST_CANCELLED         -> consumed, do not destroy
     *
     * INVALID_ARGUMENT means ownership was not transferred. This should
     * not occur for a request delivered to this route callback.
     */
    rc = mma_async_request_complete(request, response);
    if (rc == MMA_INVALID_ARGUMENT) {
        mma_response_destroy(response);
    } else if (rc != MMA_OK &&
               rc != MMA_REQUEST_ALREADY_COMPLETED &&
               rc != MMA_REQUEST_CANCELLED) {
        fprintf(stderr, "mma_async_request_complete: %d\n", (int)rc);
    }
}

static void echo_route(
    MMA_AsyncRequest *request,
    void *user_data)
{
    (void)user_data;

    MMA_RequestInfo info;
    int32_t rc = mma_async_request_get_info(request, &info);
    if (rc != MMA_OK) {
        fprintf(stderr, "echo: get_info failed: %d\n", (int)rc);
        return;
    }

    complete_request(
        request,
        info.request_id,
        info.payload,
        info.payload_len
    );
}

static void health_route(
    MMA_AsyncRequest *request,
    void *user_data)
{
    (void)user_data;

    MMA_RequestInfo info;
    int32_t rc = mma_async_request_get_info(request, &info);
    if (rc != MMA_OK) {
        fprintf(stderr, "health: get_info failed: %d\n", (int)rc);
        return;
    }

    static const uint8_t body[] = "ok";
    complete_request(
        request,
        info.request_id,
        body,
        sizeof(body) - 1
    );
}

/*
 * Channel callbacks are notifications, not request/response routes.
 * Do not call mma_async_request_complete() here.
 */
static void chat_channel(
    MMA_AsyncRequest *request,
    uint32_t connection_id,
    void *user_data)
{
    (void)user_data;

    MMA_RequestInfo info;
    int32_t rc = mma_async_request_get_info(request, &info);
    if (rc != MMA_OK) {
        fprintf(stderr, "channel %u: get_info failed: %d\n",
                (unsigned)connection_id, (int)rc);
        return;
    }

    fprintf(stdout, "[channel %u] ", (unsigned)connection_id);
    if (info.payload_len != 0) {
        (void)fwrite(info.payload, 1, info.payload_len, stdout);
    }
    fputc('\n', stdout);
    fflush(stdout);
}

static void channel_connected(
    MMA_AsyncRequest *request,
    uint32_t connection_id,
    void *user_data)
{
    (void)request;
    (void)user_data;
    fprintf(stdout, "[channel] connected: %u\n",
            (unsigned)connection_id);
    fflush(stdout);
}

static void channel_disconnected(
    MMA_AsyncRequest *request,
    uint32_t connection_id,
    void *user_data)
{
    (void)request;
    (void)user_data;
    fprintf(stdout, "[channel] disconnected: %u\n",
            (unsigned)connection_id);
    fflush(stdout);
}

static int parse_port(const char *text, uint16_t *port_out)
{
    char *end = NULL;
    errno = 0;
    unsigned long value = strtoul(text, &end, 10);

    if (errno != 0 || end == text || *end != '\0' ||
        value == 0 || value > UINT16_MAX) {
        return 0;
    }

    *port_out = (uint16_t)value;
    return 1;
}

static int run_server(
    uint16_t port,
    const MMA_TLSConfig *tls_config)
{
    MMA_ServerConfig config;
    int32_t rc = mma_server_config_default(&config);
    if (rc != MMA_OK) {
        fprintf(stderr, "mma_server_config_default: %d\n", (int)rc);
        return 1;
    }

    config.bind_ip = "0.0.0.0";
    config.port = port;
    config.tls_config = tls_config;

    MMA_Server *server = NULL;
    rc = mma_server_create(&config, &server);
    if (rc != MMA_OK || server == NULL) {
        fprintf(stderr, "mma_server_create: %d\n", (int)rc);
        return 1;
    }

    int started = 0;
    int exit_code = 1;
    uint16_t actual_port = port;
    struct sigaction action;

    rc = register_route(server, "echo", echo_route);
    if (rc != MMA_OK) {
        fprintf(stderr, "register route echo: %d\n", (int)rc);
        goto cleanup;
    }

    rc = register_route(server, "health", health_route);
    if (rc != MMA_OK) {
        fprintf(stderr, "register route health: %d\n", (int)rc);
        goto cleanup;
    }

    rc = mma_server_register_async_channel_handler(
        server,
        (const uint8_t *)"chat",
        sizeof("chat") - 1,
        chat_channel,
        NULL
    );
    if (rc != MMA_OK) {
        fprintf(stderr, "register channel handler chat: %d\n", (int)rc);
        goto cleanup;
    }

    rc = mma_server_register_async_on_channel_connect_handler(
        server, channel_connected, NULL
    );
    if (rc != MMA_OK) {
        fprintf(stderr, "register connect handler: %d\n", (int)rc);
        goto cleanup;
    }

    rc = mma_server_register_async_on_channel_disconnect_handler(
        server, channel_disconnected, NULL
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

    rc = mma_server_local_port(server, &actual_port);
    if (rc != MMA_OK) {
        fprintf(stderr, "mma_server_local_port: %d\n", (int)rc);
        goto cleanup;
    }

    memset(&action, 0, sizeof(action));
    action.sa_handler = handle_signal;
    sigemptyset(&action.sa_mask);
    (void)sigaction(SIGINT, &action, NULL);
    (void)sigaction(SIGTERM, &action, NULL);

    printf("MMA server listening on %s:%u\n",
           config.bind_ip, (unsigned)actual_port);
    printf("Routes: echo, health; channel handler: chat\n");
    printf("Press Ctrl+C to stop.\n");
    fflush(stdout);

    while (!g_stop_requested) {
        (void)sleep(1);
    }

    exit_code = 0;

cleanup:
    if (started) {
        int32_t stop_rc = mma_server_stop(server);
        if (stop_rc != MMA_OK) {
            fprintf(stderr, "mma_server_stop: %d\n", (int)stop_rc);
            exit_code = 1;
        }
    }

    mma_server_destroy(server);
    return exit_code;
}

int main(int argc, char **argv)
{
    uint16_t port = 9090;

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

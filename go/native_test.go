//go:build native_integration

package cabi

import (
	"testing"
	"time"
)

func TestDefaultServerConfigMirrorsNativeDefaults(t *testing.T) {
	config, err := DefaultServerConfig()
	if err != nil {
		t.Fatal(err)
	}
	if config.BindIP != "0.0.0.0" || config.Port != 8080 {
		t.Fatalf("unexpected bind defaults: %s:%d", config.BindIP, config.Port)
	}
	if config.MaxBatch != 16 || config.MaxInFlight != 1024 || config.MaxConnections != 128000 {
		t.Fatalf("unexpected concurrency defaults: %+v", config)
	}
	if config.DrainTimeout != 2*time.Second || config.ShutdownGrace != 10*time.Second || config.RequestTimeout != 10*time.Second {
		t.Fatalf("unexpected timeout defaults: %+v", config)
	}
	if config.Framer.HeaderLength != 29 || config.Framer.BigBytesConfig != (BigBytesConfig{OptionsLenOrder: 1, PayloadLenOrder: 2, ReqIDOrder: 3}) {
		t.Fatalf("unexpected framer defaults: %+v", config.Framer)
	}
}

// This test exercises the current C function signatures and callback pointer
// registration without binding a TCP port. The actual callback invocation is
// covered by application-level end-to-end tests.
func TestCreateServerAndRegisterCallbacks(t *testing.T) {
	config, err := DefaultServerConfig()
	if err != nil {
		t.Fatal(err)
	}

	server, err := NewServer(config)
	if err != nil {
		t.Fatalf("NewServer: %v", err)
	}
	defer func() {
		if err := server.Close(); err != nil {
			t.Errorf("Server.Close: %v", err)
		}
	}()

	if err := server.RegisterRoute("go_echo", func(request *Request, response *ResponseBuilder) error {
		if err := response.SetStatus(StatusOK); err != nil {
			return err
		}
		return response.SetPayload(request.Payload)
	}); err != nil {
		t.Fatalf("RegisterRoute: %v", err)
	}

	if err := server.RegisterChannelHandler("go_chat", func(*Request, uint32) error {
		return nil
	}); err != nil {
		t.Fatalf("RegisterChannelHandler: %v", err)
	}

	if err := server.RegisterOnChannelConnectHandler(func(*Request, uint32) error {
		return nil
	}); err != nil {
		t.Fatalf("RegisterOnChannelConnectHandler: %v", err)
	}

	if err := server.RegisterOnChannelDisconnectHandler(func(*Request, uint32) error {
		return nil
	}); err != nil {
		t.Fatalf("RegisterOnChannelDisconnectHandler: %v", err)
	}
}

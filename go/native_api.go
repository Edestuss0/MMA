package cabi

import (
	"fmt"
	"path/filepath"
	"runtime"
	"sync"
	"unsafe"

	"github.com/ebitengine/purego"
)

type nativeAPI struct {
	library uintptr

	serverConfigDefault      func(unsafe.Pointer) int32
	serverCreate             func(unsafe.Pointer, unsafe.Pointer) int32
	serverRegisterRoute      func(unsafe.Pointer, unsafe.Pointer, uintptr, uintptr, uintptr) int32
	serverRegisterChannel    func(unsafe.Pointer, unsafe.Pointer, uintptr, uintptr, uintptr) int32
	serverRegisterConnect    func(unsafe.Pointer, uintptr, uintptr) int32
	serverRegisterDisconnect func(unsafe.Pointer, uintptr, uintptr) int32
	serverStart              func(unsafe.Pointer) int32
	serverLocalPort          func(unsafe.Pointer, unsafe.Pointer) int32
	serverStop               func(unsafe.Pointer) int32
	serverDestroy            func(unsafe.Pointer)

	serverGetHandle           func(unsafe.Pointer, unsafe.Pointer) int32
	serverHandleDestroy       func(unsafe.Pointer)
	serverHandleSendToChannel func(unsafe.Pointer, uint32, unsafe.Pointer) int32
	serverHandleChangeConfig  func(unsafe.Pointer, unsafe.Pointer) int32

	requestRetain      func(unsafe.Pointer) int32
	requestRelease     func(unsafe.Pointer)
	requestGetInfo     func(unsafe.Pointer, unsafe.Pointer) int32
	requestOptionCount func(unsafe.Pointer, unsafe.Pointer) int32
	requestOptionAt    func(unsafe.Pointer, uintptr, unsafe.Pointer, unsafe.Pointer, unsafe.Pointer, unsafe.Pointer) int32
	requestIsCancelled func(unsafe.Pointer, unsafe.Pointer) int32
	requestComplete    func(unsafe.Pointer, unsafe.Pointer) int32

	responseNew        func(uint32) unsafe.Pointer
	responseSetStatus  func(unsafe.Pointer, uint8) int32
	responseSetPayload func(unsafe.Pointer, unsafe.Pointer, uintptr) int32
	responseAddOption  func(unsafe.Pointer, unsafe.Pointer, uintptr, unsafe.Pointer, uintptr) int32
	responseDestroy    func(unsafe.Pointer)

	routeCallback   uintptr
	channelCallback uintptr
}

var (
	apiMu     sync.Mutex
	loadedAPI *nativeAPI
)

func getNativeAPI() (*nativeAPI, error) {
	apiMu.Lock()
	defer apiMu.Unlock()

	if loadedAPI != nil {
		return loadedAPI, nil
	}

	libraryPath, err := locateNativeLibrary()
	if err != nil {
		return nil, err
	}
	if runtime.GOOS == "windows" {
		addDLLSearchPath(filepath.Dir(libraryPath))
	}

	library, err := openNativeLibrary(libraryPath)
	if err != nil {
		return nil, fmt.Errorf("load MMA native library %q: %w", libraryPath, err)
	}

	api := &nativeAPI{library: library}
	bindings := []struct {
		name string
		fn   any
	}{
		{"mma_server_config_default", &api.serverConfigDefault},
		{"mma_server_create", &api.serverCreate},
		{"mma_server_register_route", &api.serverRegisterRoute},
		{"mma_server_register_channel_handler", &api.serverRegisterChannel},
		{"mma_server_register_on_channel_connect_handler", &api.serverRegisterConnect},
		{"mma_server_register_on_channel_disconnect_handler", &api.serverRegisterDisconnect},
		{"mma_server_start", &api.serverStart},
		{"mma_server_local_port", &api.serverLocalPort},
		{"mma_server_stop", &api.serverStop},
		{"mma_server_destroy", &api.serverDestroy},
		{"mma_server_get_handle", &api.serverGetHandle},
		{"mma_server_handle_destroy", &api.serverHandleDestroy},
		{"mma_server_handle_send_to_channel", &api.serverHandleSendToChannel},
		{"mma_server_handle_change_config", &api.serverHandleChangeConfig},
		{"mma_request_retain", &api.requestRetain},
		{"mma_request_release", &api.requestRelease},
		{"mma_request_get_info", &api.requestGetInfo},
		{"mma_request_option_count", &api.requestOptionCount},
		{"mma_request_option_at", &api.requestOptionAt},
		{"mma_request_is_cancelled", &api.requestIsCancelled},
		{"mma_request_complete", &api.requestComplete},
		{"mma_response_new", &api.responseNew},
		{"mma_response_set_status", &api.responseSetStatus},
		{"mma_response_set_payload", &api.responseSetPayload},
		{"mma_response_add_option", &api.responseAddOption},
		{"mma_response_destroy", &api.responseDestroy},
	}

	for _, binding := range bindings {
		if err := bindNativeFunction(library, binding.name, binding.fn); err != nil {
			_ = closeNativeLibrary(library)
			return nil, err
		}
	}

	var callbackErr error
	api.routeCallback, callbackErr = createNativeCallback(nativeRouteCallback)
	if callbackErr != nil {
		_ = closeNativeLibrary(library)
		return nil, fmt.Errorf("create MMA route callback: %w", callbackErr)
	}
	api.channelCallback, callbackErr = createNativeCallback(nativeChannelCallback)
	if callbackErr != nil {
		_ = closeNativeLibrary(library)
		return nil, fmt.Errorf("create MMA channel callback: %w", callbackErr)
	}

	// The callback trampolines and the native library remain loaded for the
	// lifetime of the process. purego callback trampolines are not reclaimable.
	loadedAPI = api
	return api, nil
}

func bindNativeFunction(library uintptr, name string, fn any) (err error) {
	address, err := lookupNativeSymbol(library, name)
	if err != nil {
		return fmt.Errorf("resolve MMA symbol %q: %w", name, err)
	}
	defer func() {
		if recovered := recover(); recovered != nil {
			err = fmt.Errorf("bind MMA symbol %q: %v", name, recovered)
		}
	}()
	purego.RegisterFunc(fn, address)
	return nil
}

func createNativeCallback(fn any) (callback uintptr, err error) {
	defer func() {
		if recovered := recover(); recovered != nil {
			err = fmt.Errorf("purego callback ABI is unsupported on %s/%s: %v", runtime.GOOS, runtime.GOARCH, recovered)
		}
	}()
	return purego.NewCallback(fn), nil
}

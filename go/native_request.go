package cabi

import (
	"fmt"
	"log"
	"runtime"
	"sync"
	"sync/atomic"
	"unsafe"

	"github.com/ebitengine/purego"
)

type routeRegistration struct {
	api     *nativeAPI
	handler RouteHandler
}

type channelRegistration struct {
	api     *nativeAPI
	handler ChannelHandler
}

var (
	nextCallbackID atomic.Uint64
	callbackTable  sync.Map // uintptr -> routeRegistration or channelRegistration
)

func keepCallback(callback any) uintptr {
	for {
		id := uintptr(nextCallbackID.Add(1))
		if id == 0 {
			continue
		}
		if _, loaded := callbackTable.LoadOrStore(id, callback); !loaded {
			return id
		}
	}
}

func releaseCallback(id uintptr) { callbackTable.Delete(id) }

func callbackByID(id uintptr) (any, bool) { return callbackTable.Load(id) }

func readNativeRequestInfo(api *nativeAPI, request unsafe.Pointer) (nativeRequestInfo, error) {
	if request == nil {
		return nativeRequestInfo{}, ErrInvalidArgument
	}
	var info nativeRequestInfo
	if err := statusError(api.requestGetInfo(request, unsafe.Pointer(&info))); err != nil {
		return nativeRequestInfo{}, err
	}
	runtime.KeepAlive(&info)
	return info, nil
}

func requestFromNative(api *nativeAPI, request unsafe.Pointer) (*Request, error) {
	info, err := readNativeRequestInfo(api, request)
	if err != nil {
		return nil, err
	}
	return requestFromNativeInfo(api, request, info)
}

func requestFromNativeInfo(api *nativeAPI, request unsafe.Pointer, info nativeRequestInfo) (*Request, error) {
	route, err := copyNativeBytes(info.route, info.routeLen)
	if err != nil {
		return nil, err
	}
	payload, err := copyNativeBytes(info.payload, info.payloadLen)
	if err != nil {
		return nil, err
	}

	var nativeCount uintptr
	if err := statusError(api.requestOptionCount(request, unsafe.Pointer(&nativeCount))); err != nil {
		return nil, err
	}
	count, err := nativeSizeToInt(nativeCount)
	if err != nil || count > 255 {
		return nil, ErrInvalidArgument
	}

	options := make([]Option, 0, count)
	for index := 0; index < count; index++ {
		var keyPointer unsafe.Pointer
		var keyLength uintptr
		var valuePointer unsafe.Pointer
		var valueLength uintptr

		if err := statusError(api.requestOptionAt(
			request,
			uintptr(index),
			unsafe.Pointer(&keyPointer),
			unsafe.Pointer(&keyLength),
			unsafe.Pointer(&valuePointer),
			unsafe.Pointer(&valueLength),
		)); err != nil {
			return nil, err
		}

		key, err := copyNativeBytes(keyPointer, keyLength)
		if err != nil {
			return nil, err
		}
		value, err := copyNativeBytes(valuePointer, valueLength)
		if err != nil {
			return nil, err
		}
		options = append(options, Option{Key: string(key), Value: string(value)})
	}

	return &Request{
		Opcode:  RequestOpcode(info.opcode),
		Version: info.version,
		ReqID:   info.requestID,
		Route:   string(route),
		Options: options,
		Payload: payload,
	}, nil
}

func copyNativeBytes(pointer unsafe.Pointer, size uintptr) ([]byte, error) {
	n, err := nativeSizeToInt(size)
	if err != nil {
		return nil, err
	}
	if n == 0 {
		return nil, nil
	}
	if pointer == nil {
		return nil, ErrInvalidArgument
	}
	result := make([]byte, n)
	copy(result, unsafe.Slice((*byte)(pointer), n))
	return result, nil
}

func nativeSizeToInt(size uintptr) (int, error) {
	maxInt := uintptr(^uint(0) >> 1)
	if size > maxInt {
		return 0, fmt.Errorf("native size_t value does not fit into Go int")
	}
	return int(size), nil
}

// nativeRouteCallback is a C-ABI callback. It only retains the native request
// and schedules Go work; user code never runs on a Tokio worker thread.
func nativeRouteCallback(_ purego.CDecl, request unsafe.Pointer, userData uintptr) {
	defer func() {
		if recovered := recover(); recovered != nil {
			log.Printf("MMA Go route callback panic: %v", recovered)
		}
	}()
	if request == nil {
		log.Printf("MMA Go route callback received a nil request")
		return
	}

	value, found := callbackByID(userData)
	registration, valid := value.(routeRegistration)
	if !found || !valid || registration.api == nil || registration.handler == nil {
		log.Printf("MMA Go route callback registration %d is unavailable", userData)
		return
	}

	if err := statusError(registration.api.requestRetain(request)); err != nil {
		log.Printf("MMA request retain failed: %v", err)
		return
	}

	go dispatchRoute(registration, request)
}

func dispatchRoute(registration routeRegistration, request unsafe.Pointer) {
	api := registration.api
	defer api.requestRelease(request)

	info, infoErr := readNativeRequestInfo(api, request)
	requestID := uint32(0)
	if infoErr == nil {
		requestID = info.requestID
	}

	goRequest, requestErr := (*Request)(nil), infoErr
	if requestErr == nil {
		goRequest, requestErr = requestFromNativeInfo(api, request, info)
	}

	response := api.responseNew(requestID)
	if response == nil {
		log.Printf("MMA could not allocate a response for request %d", requestID)
		return
	}
	builder := &ResponseBuilder{api: api, response: response}
	responseOwned := true
	defer func() {
		if responseOwned {
			api.responseDestroy(response)
		}
		builder.response = nil
	}()

	if requestErr != nil {
		_ = builder.SetStatus(StatusBadRequest)
		_ = builder.SetPayload([]byte("Invalid request"))
		completeNativeResponse(api, request, response, &responseOwned)
		return
	}

	if isNativeRequestCancelled(api, request) {
		return
	}

	if err := invokeRouteHandler(registration.handler, goRequest, builder); err != nil {
		log.Printf("MMA Go route handler failed for %q: %v", goRequest.Route, err)
		_ = builder.SetStatus(StatusInternalError)
		_ = builder.SetPayload([]byte("Internal server error"))
	}

	completeNativeResponse(api, request, response, &responseOwned)
}

func invokeRouteHandler(handler RouteHandler, request *Request, response *ResponseBuilder) (err error) {
	defer func() {
		if recovered := recover(); recovered != nil {
			err = fmt.Errorf("handler panic: %v", recovered)
		}
	}()
	return handler(request, response)
}

func completeNativeResponse(api *nativeAPI, request, response unsafe.Pointer, owned *bool) {
	status := api.requestComplete(request, response)
	if status == MMAInvalidArgument {
		// The C ABI explicitly leaves ownership with the caller only for
		// INVALID_ARGUMENT (for example, a non-response notification).
		api.responseDestroy(response)
		*owned = false
		log.Printf("MMA request completion rejected the response: %v", statusError(status))
		return
	}

	// For every other result the Rust ABI consumes the response, including
	// already-completed and cancelled requests.
	*owned = false
	if status != MMAOK && status != MMARequestAlreadyCompleted && status != MMARequestCancelled {
		log.Printf("MMA request completion failed: %v", statusError(status))
	}
}

func isNativeRequestCancelled(api *nativeAPI, request unsafe.Pointer) bool {
	var cancelled uint8
	if err := statusError(api.requestIsCancelled(request, unsafe.Pointer(&cancelled))); err != nil {
		return false
	}
	runtime.KeepAlive(&cancelled)
	return cancelled != 0
}

// nativeChannelCallback is shared by channel-message and lifecycle callbacks.
// The request is copied and the handler is invoked inline so messages from a
// single TCP connection retain their delivery order. Channel handlers should
// therefore be short and non-blocking; they can enqueue the copied Request to
// an application-owned worker and return if more work is required.
func nativeChannelCallback(
	_ purego.CDecl,
	request unsafe.Pointer,
	connectionID uint32,
	userData uintptr,
) {
	defer func() {
		if recovered := recover(); recovered != nil {
			log.Printf("MMA Go channel callback panic: %v", recovered)
		}
	}()
	if request == nil {
		log.Printf("MMA Go channel callback received a nil request")
		return
	}

	value, found := callbackByID(userData)
	registration, valid := value.(channelRegistration)
	if !found || !valid || registration.api == nil || registration.handler == nil {
		log.Printf("MMA Go channel callback registration %d is unavailable", userData)
		return
	}

	// The C callback owns a live Arc while this function is running. All
	// request buffers are copied before returning to Rust, so retain is not
	// necessary for this synchronous channel/lifecycle path.
	goRequest, err := requestFromNative(registration.api, request)
	if err != nil {
		log.Printf("MMA channel request decoding failed: %v", err)
		return
	}
	if err := invokeChannelHandler(registration.handler, goRequest, connectionID); err != nil {
		log.Printf("MMA Go channel handler failed for connection %d: %v", connectionID, err)
	}
}

func invokeChannelHandler(handler ChannelHandler, request *Request, connectionID uint32) (err error) {
	defer func() {
		if recovered := recover(); recovered != nil {
			err = fmt.Errorf("handler panic: %v", recovered)
		}
	}()
	return handler(request, connectionID)
}

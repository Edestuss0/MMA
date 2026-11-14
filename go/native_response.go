package cabi

import (
	"fmt"
	"runtime"
	"strings"
	"unsafe"
)

// ResponseBuilder updates a native response object. It is valid only while
// the corresponding RouteHandler is executing.
type ResponseBuilder struct {
	api      *nativeAPI
	response unsafe.Pointer
}

// SetStatus accepts a legacy optional version argument. The current native
// C ABI assigns the response protocol version itself, so the argument is
// intentionally ignored for source compatibility.
func (b *ResponseBuilder) SetStatus(opcode ResponseOpcode, _ ...uint8) error {
	if b == nil || b.api == nil || b.response == nil {
		return ErrInvalidArgument
	}
	return statusError(b.api.responseSetStatus(b.response, uint8(opcode)))
}

func (b *ResponseBuilder) SetStatusBuilder(opcode ResponseOpcode, version ...uint8) error {
	return b.SetStatus(opcode, version...)
}

func (b *ResponseBuilder) SetPayload(payload []byte) error {
	if b == nil || b.api == nil || b.response == nil {
		return ErrInvalidArgument
	}
	err := statusError(b.api.responseSetPayload(
		b.response,
		byteSlicePointer(payload),
		uintptr(len(payload)),
	))
	runtime.KeepAlive(payload)
	return err
}

func (b *ResponseBuilder) AddOption(key, value string) error {
	if b == nil || b.api == nil || b.response == nil {
		return ErrInvalidArgument
	}
	if key == "" || value == "" || strings.IndexByte(key, 0) >= 0 || strings.IndexByte(value, 0) >= 0 {
		return fmt.Errorf("%w: option key and value must be non-empty and cannot contain NUL", ErrInvalidArgument)
	}
	keyBytes := []byte(key)
	valueBytes := []byte(value)
	if len(keyBytes) > 65535 || len(valueBytes) > 65535 {
		return fmt.Errorf("%w: option key and value must not exceed 65535 bytes", ErrInvalidArgument)
	}
	err := statusError(b.api.responseAddOption(
		b.response,
		byteSlicePointer(keyBytes), uintptr(len(keyBytes)),
		byteSlicePointer(valueBytes), uintptr(len(valueBytes)),
	))
	runtime.KeepAlive(keyBytes)
	runtime.KeepAlive(valueBytes)
	return err
}

func buildNativeResponse(api *nativeAPI, message Response) (unsafe.Pointer, error) {
	response := api.responseNew(message.ReqID)
	if response == nil {
		return nil, fmt.Errorf("mma_response_new returned a nil response")
	}
	builder := &ResponseBuilder{api: api, response: response}
	owned := true
	defer func() {
		if owned {
			api.responseDestroy(response)
		}
	}()

	opcode := message.Opcode
	if opcode == 0 {
		opcode = StatusMessage
	}
	if err := builder.SetStatus(opcode); err != nil {
		return nil, err
	}
	if err := builder.SetPayload(message.Payload); err != nil {
		return nil, err
	}
	for _, option := range message.Options {
		if err := builder.AddOption(option.Key, option.Value); err != nil {
			return nil, err
		}
	}

	owned = false
	return response, nil
}

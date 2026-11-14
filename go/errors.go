package cabi

import (
	"errors"
	"fmt"
)

var (
	ErrIncomplete              = errors.New("frame is incomplete")
	ErrInvalidArgument         = errors.New("invalid argument")
	ErrInvalidConfig           = errors.New("invalid config")
	ErrEncode                  = errors.New("encode failed")
	ErrDecode                  = errors.New("decode failed")
	ErrServer                  = errors.New("server operation failed")
	ErrServerRunning           = errors.New("server is already running")
	ErrCallback                = errors.New("route callback failed")
	ErrClosed                  = errors.New("object is closed")
	ErrRequestAlreadyCompleted = errors.New("request is already completed")
	ErrRequestCancelled        = errors.New("request was cancelled or timed out")
	// Deprecated: purego does not require CGO. Kept for source compatibility.
	ErrCGODisabled = errors.New("legacy CGO wrapper is disabled")
)

type StatusError struct {
	Code    int32
	Message string
}

func (e *StatusError) Error() string {
	if e == nil {
		return "<nil>"
	}
	if e.Message == "" {
		return fmt.Sprintf("MMA C ABI status %d", e.Code)
	}
	return fmt.Sprintf("MMA C ABI status %d: %s", e.Code, e.Message)
}

func (e *StatusError) Unwrap() error {
	if e == nil {
		return nil
	}
	switch e.Code {
	case MMAInvalidArgument:
		return ErrInvalidArgument
	case MMAInvalidConfig:
		return ErrInvalidConfig
	case MMAServerError:
		return ErrServer
	case MMAServerAlreadyStarted:
		return ErrServerRunning
	case MMARequestAlreadyCompleted:
		return ErrRequestAlreadyCompleted
	case MMARequestCancelled:
		return ErrRequestCancelled
	default:
		return nil
	}
}

const (
	MMAOK                      int32 = 0
	MMAInvalidArgument         int32 = -1
	MMAInvalidConfig           int32 = -2
	MMAServerError             int32 = -3
	MMAServerAlreadyStarted    int32 = -4
	MMARequestAlreadyCompleted int32 = -5
	MMARequestCancelled        int32 = -6
)

func statusError(code int32) error {
	if code == MMAOK {
		return nil
	}
	message := "unknown status"
	switch code {
	case MMAInvalidArgument:
		message = "invalid argument"
	case MMAInvalidConfig:
		message = "invalid configuration"
	case MMAServerError:
		message = "server operation failed"
	case MMAServerAlreadyStarted:
		message = "server is already started"
	case MMARequestAlreadyCompleted:
		message = "request is already completed"
	case MMARequestCancelled:
		message = "request was cancelled or timed out"
	}
	return &StatusError{Code: code, Message: message}
}

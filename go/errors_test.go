package cabi

import (
	"errors"
	"fmt"
	"testing"
)

func TestStatusErrorUnwrap(t *testing.T) {
	tests := []struct {
		code int32
		want error
	}{
		{MMAInvalidArgument, ErrInvalidArgument},
		{MMAInvalidConfig, ErrInvalidConfig},
		{MMAServerError, ErrServer},
		{MMAServerAlreadyStarted, ErrServerRunning},
		{MMARequestAlreadyCompleted, ErrRequestAlreadyCompleted},
		{MMARequestCancelled, ErrRequestCancelled},
		{12345, nil},
	}

	for _, test := range tests {
		t.Run(fmt.Sprintf("code_%d", test.code), func(t *testing.T) {
			err := (&StatusError{Code: test.code}).Unwrap()
			if !errors.Is(err, test.want) {
				if err == nil && test.want == nil {
					return
				}
				t.Fatalf("Unwrap() = %v, want %v", err, test.want)
			}
		})
	}
}

func TestStatusError(t *testing.T) {
	if err := statusError(MMAOK); err != nil {
		t.Fatalf("statusError(MMA_OK) = %v, want nil", err)
	}
	if err := statusError(MMARequestCancelled); !errors.Is(err, ErrRequestCancelled) {
		t.Fatalf("statusError(MMA_REQUEST_CANCELLED) = %v", err)
	}
}

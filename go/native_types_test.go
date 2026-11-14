package cabi

import (
	"testing"
	"unsafe"
)

// The published native binaries currently target 64-bit Windows, Linux, and
// macOS. These tests keep Go's purego mirrors synchronized with mma.h.
func TestNativeABILayouts64Bit(t *testing.T) {
	if unsafe.Sizeof(uintptr(0)) != 8 {
		t.Skip("native release artifacts currently target 64-bit platforms")
	}

	tests := []struct {
		name string
		got  uintptr
		want uintptr
	}{
		{"MMA_BigBytesConfig", unsafe.Sizeof(nativeBigBytesConfig{}), 3},
		{"MMA_FramerConfig", unsafe.Sizeof(nativeFramerConfig{}), 16},
		{"MMA_TLSConfig", unsafe.Sizeof(nativeTLSConfig{}), 24},
		{"MMA_RequestInfo", unsafe.Sizeof(nativeRequestInfo{}), 40},
		{"MMA_ServerConfig", unsafe.Sizeof(nativeServerConfig{}), 72},
	}

	for _, test := range tests {
		if test.got != test.want {
			t.Errorf("sizeof(%s) = %d, want %d", test.name, test.got, test.want)
		}
	}
}

func TestNativeABILayoutOffsets64Bit(t *testing.T) {
	if unsafe.Sizeof(uintptr(0)) != 8 {
		t.Skip("native release artifacts currently target 64-bit platforms")
	}

	if got := unsafe.Offsetof(nativeServerConfig{}.tlsConfig); got != 64 {
		t.Errorf("MMA_ServerConfig.tls_config offset = %d, want 64", got)
	}
	if got := unsafe.Offsetof(nativeRequestInfo{}.requestID); got != 20 {
		t.Errorf("MMA_RequestInfo.request_id offset = %d, want 20", got)
	}
	if got := unsafe.Offsetof(nativeRequestInfo{}.payload); got != 24 {
		t.Errorf("MMA_RequestInfo.payload offset = %d, want 24", got)
	}
	if got := unsafe.Offsetof(nativeRequestInfo{}.payloadLen); got != 32 {
		t.Errorf("MMA_RequestInfo.payload_len offset = %d, want 32", got)
	}
}

package cabi

import "unsafe"

// These types mirror the #[repr(C)] declarations in include/mma.h.
// Do not expose them from the public Go API.
type nativeBigBytesConfig struct {
	optionsLenOrder uint8
	payloadLenOrder uint8
	reqIDOrder      uint8
}

type nativeFramerConfig struct {
	maxMessageLength uint32
	opcodePos        uint8
	versionPos       uint8
	routeLenPos      uint8
	routeOrder       uint8
	payloadOrder     uint8
	optionsOrder     uint8
	optionsCountPos  uint8
	optionsKeyFirst  uint8
	headerLength     uint8
	bigBytesConfig   nativeBigBytesConfig
}

type nativeTLSConfig struct {
	certificatePath    unsafe.Pointer
	keyPath            unsafe.Pointer
	handshakeTimeoutMS uint64
}

type nativeServerConfig struct {
	bindIP           unsafe.Pointer
	port             uint16
	maxBatch         uint32
	maxInFlight      uint32
	framer           nativeFramerConfig
	maxConnections   uint32
	drainTimeoutMS   uint64
	shutdownGraceMS  uint64
	requestTimeoutMS uint64
	tlsConfig        unsafe.Pointer
}

type nativeRequestInfo struct {
	route      unsafe.Pointer
	routeLen   uintptr
	opcode     uint8
	version    uint8
	requestID  uint32
	payload    unsafe.Pointer
	payloadLen uintptr
}

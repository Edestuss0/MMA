// Package cabi provides Go bindings for the MMA native C ABI.
//
// The public Go types intentionally remain independent from the native C
// structs. Native request and response objects are opaque; this package copies
// request data into Go-owned memory and builds responses through C ABI calls.
package cabi

import "time"

type RequestOpcode uint8

const (
	OpcodeChannel        RequestOpcode = 1 // Channel connection request.
	OpcodeOnce           RequestOpcode = 2 // One-shot request.
	OpcodeChannelMessage RequestOpcode = 3 // Message sent on an established channel.
)

type ResponseOpcode uint8

const (
	StatusOK               ResponseOpcode = 1
	StatusBadRequest       ResponseOpcode = 2
	StatusUnauthorized     ResponseOpcode = 3
	StatusForbidden        ResponseOpcode = 4
	StatusNotFound         ResponseOpcode = 5
	StatusConflict         ResponseOpcode = 6
	StatusInternalError    ResponseOpcode = 7
	StatusMessage          ResponseOpcode = 8
	StatusShutdown         ResponseOpcode = 9
	StatusChannelConnected ResponseOpcode = 10
	StatusTimeout          ResponseOpcode = 11
)

type Option struct {
	Key   string
	Value string
}

type Request struct {
	Opcode  RequestOpcode
	Version uint8
	ReqID   uint32
	Route   string
	Options []Option
	Payload []byte
}

type Response struct {
	Opcode  ResponseOpcode
	Version uint8 // The native server currently sets the protocol version itself.
	ReqID   uint32
	Options []Option
	Payload []byte
}

type BigBytesConfig struct {
	OptionsLenOrder uint8
	PayloadLenOrder uint8
	ReqIDOrder      uint8
}

type FramerConfig struct {
	MaxMessageLength uint32
	OpcodePos        uint8
	VersionPos       uint8
	RouteLenPos      uint8
	RouteOrder       uint8
	PayloadOrder     uint8
	OptionsOrder     uint8
	OptionsCountPos  uint8
	OptionsKeyFirst  bool
	HeaderLength     uint8
	BigBytesConfig   BigBytesConfig
}

type ServerConfig struct {
	BindIP         string
	Port           uint16
	MaxBatch       uint32
	MaxInFlight    uint32
	Framer         FramerConfig
	MaxConnections uint32
	DrainTimeout   time.Duration
	ShutdownGrace  time.Duration
	RequestTimeout time.Duration
}

// TLSConfig enables TLS without changing the existing ServerConfig layout.
// This is an additive API: applications that do not use TLS can continue to
// construct ServerConfig exactly as before.
type TLSConfig struct {
	CertificatePath  string
	KeyPath          string
	HandshakeTimeout time.Duration
}

// RouteHandler handles a one-shot request and mutates its response.
// The ResponseBuilder is valid only for the duration of the handler call and
// must not be retained or used concurrently after the handler returns.
type RouteHandler func(request *Request, response *ResponseBuilder) error

// ChannelHandler handles a message or lifecycle notification delivered for a
// channel connection. Channel handlers do not produce a request response.
type ChannelHandler func(request *Request, connectionID uint32) error

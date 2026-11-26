package cabi

import (
	"fmt"
	"runtime"
	"strings"
	"time"
	"unsafe"
)

// DefaultServerConfig returns defaults supplied by the Rust C ABI.
func DefaultServerConfig() (ServerConfig, error) {
	api, err := getNativeAPI()
	if err != nil {
		return ServerConfig{}, err
	}
	var native nativeServerConfig
	if err := statusError(api.serverConfigDefault(unsafe.Pointer(&native))); err != nil {
		return ServerConfig{}, err
	}
	runtime.KeepAlive(&native)
	return serverConfigFromNative(native), nil
}

// DefaultFramerConfig returns the native default framer settings.
func DefaultFramerConfig() (FramerConfig, error) {
	config, err := DefaultServerConfig()
	if err != nil {
		return FramerConfig{}, err
	}
	return config.Framer, nil
}

func serverConfigFromNative(native nativeServerConfig) ServerConfig {
	return ServerConfig{
		BindIP:         cString(native.bindIP),
		Port:           native.port,
		MaxBatch:       native.maxBatch,
		MaxInFlight:    native.maxInFlight,
		Framer:         framerConfigFromNative(native.framer),
		MaxConnections: native.maxConnections,
		DrainTimeout:   time.Duration(native.drainTimeoutMS) * time.Millisecond,
		ShutdownGrace:  time.Duration(native.shutdownGraceMS) * time.Millisecond,
		RequestTimeout: time.Duration(native.requestTimeoutMS) * time.Millisecond,
		WriteTimeout:   time.Duration(native.writeTimeoutMS) * time.Millisecond,
		SocketBackoff:  time.Duration(native.socketBackoffUS) * time.Microsecond,
	}
}

func framerConfigFromNative(native nativeFramerConfig) FramerConfig {
	return FramerConfig{
		MaxMessageLength: native.maxMessageLength,
		OpcodePos:        native.opcodePos,
		VersionPos:       native.versionPos,
		RouteLenPos:      native.routeLenPos,
		RouteOrder:       native.routeOrder,
		PayloadOrder:     native.payloadOrder,
		OptionsOrder:     native.optionsOrder,
		OptionsCountPos:  native.optionsCountPos,
		OptionsKeyFirst:  native.optionsKeyFirst != 0,
		HeaderLength:     native.headerLength,
		BigBytesConfig: BigBytesConfig{
			OptionsLenOrder: native.bigBytesConfig.optionsLenOrder,
			PayloadLenOrder: native.bigBytesConfig.payloadLenOrder,
			ReqIDOrder:      native.bigBytesConfig.reqIDOrder,
		},
	}
}

func framerConfigToNative(config FramerConfig) nativeFramerConfig {
	return nativeFramerConfig{
		maxMessageLength: config.MaxMessageLength,
		opcodePos:        config.OpcodePos,
		versionPos:       config.VersionPos,
		routeLenPos:      config.RouteLenPos,
		routeOrder:       config.RouteOrder,
		payloadOrder:     config.PayloadOrder,
		optionsOrder:     config.OptionsOrder,
		optionsCountPos:  config.OptionsCountPos,
		optionsKeyFirst:  boolToUint8(config.OptionsKeyFirst),
		headerLength:     config.HeaderLength,
		bigBytesConfig: nativeBigBytesConfig{
			optionsLenOrder: config.BigBytesConfig.OptionsLenOrder,
			payloadLenOrder: config.BigBytesConfig.PayloadLenOrder,
			reqIDOrder:      config.BigBytesConfig.ReqIDOrder,
		},
	}
}

func buildNativeConfig(
	config ServerConfig,
	tls *TLSConfig,
) (nativeServerConfig, []byte, *nativeTLSConfig, []byte, []byte, error) {
	if config.BindIP == "" || strings.IndexByte(config.BindIP, 0) >= 0 {
		return nativeServerConfig{}, nil, nil, nil, nil,
			fmt.Errorf("%w: BindIP must be non-empty and cannot contain NUL", ErrInvalidArgument)
	}
	if config.DrainTimeout < 0 || config.ShutdownGrace < 0 || config.RequestTimeout < 0 || config.WriteTimeout < 0 || config.SocketBackoff < 0 {
		return nativeServerConfig{}, nil, nil, nil, nil,
			fmt.Errorf("%w: server timeouts must not be negative", ErrInvalidConfig)
	}

	bindIP := append([]byte(config.BindIP), 0)
	native := nativeServerConfig{
		bindIP:           byteSlicePointer(bindIP),
		port:             config.Port,
		maxBatch:         config.MaxBatch,
		maxInFlight:      config.MaxInFlight,
		framer:           framerConfigToNative(normalizeFramerConfig(config.Framer)),
		maxConnections:   config.MaxConnections,
		drainTimeoutMS:   uint64(config.DrainTimeout / time.Millisecond),
		shutdownGraceMS:  uint64(config.ShutdownGrace / time.Millisecond),
		requestTimeoutMS: uint64(config.RequestTimeout / time.Millisecond),
		writeTimeoutMS:   uint64(config.WriteTimeout / time.Millisecond),
		socketBackoffUS:  uint64(config.SocketBackoff / time.Microsecond),
		tlsConfig:        nil,
	}

	if tls == nil {
		return native, bindIP, nil, nil, nil, nil
	}
	if tls.CertificatePath == "" || tls.KeyPath == "" ||
		strings.IndexByte(tls.CertificatePath, 0) >= 0 ||
		strings.IndexByte(tls.KeyPath, 0) >= 0 {
		return nativeServerConfig{}, nil, nil, nil, nil,
			fmt.Errorf("%w: TLS certificate and key paths must be non-empty and cannot contain NUL", ErrInvalidArgument)
	}
	if tls.HandshakeTimeout < 0 {
		return nativeServerConfig{}, nil, nil, nil, nil,
			fmt.Errorf("%w: TLS handshake timeout must not be negative", ErrInvalidConfig)
	}

	certificatePath := append([]byte(tls.CertificatePath), 0)
	keyPath := append([]byte(tls.KeyPath), 0)
	nativeTLS := &nativeTLSConfig{
		certificatePath:    byteSlicePointer(certificatePath),
		keyPath:            byteSlicePointer(keyPath),
		handshakeTimeoutMS: uint64(tls.HandshakeTimeout / time.Millisecond),
	}
	native.tlsConfig = unsafe.Pointer(nativeTLS)
	return native, bindIP, nativeTLS, certificatePath, keyPath, nil
}

func boolToUint8(value bool) uint8 {
	if value {
		return 1
	}
	return 0
}

func byteSlicePointer(data []byte) unsafe.Pointer {
	if len(data) == 0 {
		return nil
	}
	return unsafe.Pointer(&data[0])
}

func cString(pointer unsafe.Pointer) string {
	if pointer == nil {
		return ""
	}
	const maxCStringLength = 1 << 20
	for length := 0; length < maxCStringLength; length++ {
		if *(*byte)(unsafe.Add(pointer, length)) == 0 {
			return string(unsafe.Slice((*byte)(pointer), length))
		}
	}
	return ""
}

// normalizeFramerConfig supplies the native default nested ordering when it
// is omitted by the caller.
func normalizeFramerConfig(config FramerConfig) FramerConfig {
	if config.BigBytesConfig == (BigBytesConfig{}) {
		config.BigBytesConfig = BigBytesConfig{
			OptionsLenOrder: 1,
			PayloadLenOrder: 2,
			ReqIDOrder:      3,
		}
	}
	return config
}

func keepNativeConfigAlive(
	config *nativeServerConfig,
	bindIP []byte,
	tls *nativeTLSConfig,
	certificatePath []byte,
	keyPath []byte,
) {
	runtime.KeepAlive(bindIP)
	runtime.KeepAlive(certificatePath)
	runtime.KeepAlive(keyPath)
	runtime.KeepAlive(tls)
	runtime.KeepAlive(config)
}

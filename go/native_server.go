package cabi

import (
	"errors"
	"runtime"
	"strings"
	"sync"
	"unicode/utf8"
	"unsafe"
)

// Server owns an MMA_Server allocated by the Rust shared library.
type Server struct {
	mu          sync.Mutex
	lifecycleMu sync.Mutex
	api         *nativeAPI
	ptr         unsafe.Pointer
	handles     []uintptr
	started     bool
	stopping    bool
	tlsConfig   *TLSConfig
}

// NewServer creates a server without TLS. Its public signature is unchanged
// from MMA-MRC.
func NewServer(config ServerConfig) (*Server, error) {
	return newServer(config, nil)
}

// NewServerWithTLS creates a server with TLS. It is additive and does not
// change the existing ServerConfig layout or NewServer signature.
func NewServerWithTLS(config ServerConfig, tls TLSConfig) (*Server, error) {
	return newServer(config, &tls)
}

func newServer(config ServerConfig, tls *TLSConfig) (*Server, error) {
	api, err := getNativeAPI()
	if err != nil {
		return nil, err
	}

	nativeConfig, bindIP, nativeTLS, certificatePath, keyPath, err := buildNativeConfig(config, tls)
	if err != nil {
		return nil, err
	}
	var pointer unsafe.Pointer
	status := api.serverCreate(unsafe.Pointer(&nativeConfig), unsafe.Pointer(&pointer))
	keepNativeConfigAlive(&nativeConfig, bindIP, nativeTLS, certificatePath, keyPath)
	if err := statusError(status); err != nil {
		return nil, err
	}
	if pointer == nil {
		return nil, errors.New("mma_server_create returned a nil pointer")
	}

	server := &Server{
		api:       api,
		ptr:       pointer,
		tlsConfig: cloneTLSConfig(tls),
	}
	runtime.SetFinalizer(server, (*Server).finalize)
	return server, nil
}

func cloneTLSConfig(config *TLSConfig) *TLSConfig {
	if config == nil {
		return nil
	}
	copy := *config
	return &copy
}

func (s *Server) finalize() { _ = s.Close() }

// RegisterRoute registers a one-shot route. Register routes before Start.
func (s *Server) RegisterRoute(route string, handler RouteHandler) error {
	if s == nil {
		return ErrClosed
	}
	if handler == nil || !validRoute(route) {
		return ErrInvalidArgument
	}
	id := keepCallback(routeRegistration{api: s.api, handler: handler})
	return s.registerCallbackRoute(route, id, true)
}

// Handle is an alias for RegisterRoute.
func (s *Server) Handle(route string, handler RouteHandler) error {
	return s.RegisterRoute(route, handler)
}

// RegisterChannelHandler registers a handler for messages delivered to a
// channel route.
func (s *Server) RegisterChannelHandler(route string, handler ChannelHandler) error {
	if s == nil {
		return ErrClosed
	}
	if handler == nil || !validRoute(route) {
		return ErrInvalidArgument
	}
	id := keepCallback(channelRegistration{api: s.api, handler: handler})
	return s.registerCallbackRoute(route, id, false)
}

func validRoute(route string) bool {
	return len(route) > 0 && len(route) <= 255 && utf8.ValidString(route) && strings.IndexByte(route, 0) < 0
}

func (s *Server) registerCallbackRoute(route string, id uintptr, isRoute bool) error {
	routeBytes := []byte(route)
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.ptr == nil {
		releaseCallback(id)
		return ErrClosed
	}
	if s.stopping {
		releaseCallback(id)
		return ErrServer
	}

	var status int32
	if isRoute {
		status = s.api.serverRegisterRoute(
			s.ptr,
			byteSlicePointer(routeBytes),
			uintptr(len(routeBytes)),
			s.api.routeCallback,
			id,
		)
	} else {
		status = s.api.serverRegisterChannel(
			s.ptr,
			byteSlicePointer(routeBytes),
			uintptr(len(routeBytes)),
			s.api.channelCallback,
			id,
		)
	}
	runtime.KeepAlive(routeBytes)
	if err := statusError(status); err != nil {
		releaseCallback(id)
		return err
	}
	s.handles = append(s.handles, id)
	return nil
}

// RegisterOnChannelConnectHandler registers a callback for channel-connect
// notifications. The callback has the same public shape as ChannelHandler.
func (s *Server) RegisterOnChannelConnectHandler(handler ChannelHandler) error {
	return s.registerLifecycleHandler(handler, true)
}

// RegisterOnChannelDisconnectHandler registers a callback for channel-
// disconnect notifications.
func (s *Server) RegisterOnChannelDisconnectHandler(handler ChannelHandler) error {
	return s.registerLifecycleHandler(handler, false)
}

func (s *Server) registerLifecycleHandler(handler ChannelHandler, onConnect bool) error {
	if s == nil {
		return ErrClosed
	}
	if handler == nil {
		return ErrInvalidArgument
	}
	id := keepCallback(channelRegistration{api: s.api, handler: handler})

	s.mu.Lock()
	defer s.mu.Unlock()
	if s.ptr == nil {
		releaseCallback(id)
		return ErrClosed
	}
	if s.stopping {
		releaseCallback(id)
		return ErrServer
	}

	var status int32
	if onConnect {
		status = s.api.serverRegisterConnect(s.ptr, s.api.channelCallback, id)
	} else {
		status = s.api.serverRegisterDisconnect(s.ptr, s.api.channelCallback, id)
	}
	if err := statusError(status); err != nil {
		releaseCallback(id)
		return err
	}
	s.handles = append(s.handles, id)
	return nil
}

func (s *Server) Start() error {
	if s == nil {
		return ErrClosed
	}
	s.lifecycleMu.Lock()
	defer s.lifecycleMu.Unlock()

	s.mu.Lock()
	defer s.mu.Unlock()
	if s.ptr == nil {
		return ErrClosed
	}
	if s.started {
		return &StatusError{Code: MMAServerAlreadyStarted, Message: "server is already started"}
	}
	if err := statusError(s.api.serverStart(s.ptr)); err != nil {
		return err
	}
	s.started = true
	return nil
}

// BoundPort returns the listener port reported by the native server.
func (s *Server) BoundPort() (uint16, error) {
	if s == nil {
		return 0, ErrClosed
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.ptr == nil {
		return 0, ErrClosed
	}
	if s.stopping {
		return 0, ErrServer
	}
	var port uint16
	if err := statusError(s.api.serverLocalPort(s.ptr, unsafe.Pointer(&port))); err != nil {
		return 0, err
	}
	runtime.KeepAlive(&port)
	return port, nil
}

// GetHandle returns a separately owned native handle for channel sends and
// configuration updates. Call it only after Start has returned successfully.
func (s *Server) GetHandle() (*ServerHandle, error) {
	if s == nil {
		return nil, ErrClosed
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.ptr == nil {
		return nil, ErrClosed
	}
	if s.stopping {
		return nil, ErrServer
	}
	var pointer unsafe.Pointer
	if err := statusError(s.api.serverGetHandle(s.ptr, unsafe.Pointer(&pointer))); err != nil {
		return nil, err
	}
	if pointer == nil {
		return nil, errors.New("mma_server_get_handle returned a nil pointer")
	}
	handle := &ServerHandle{
		api:       s.api,
		ptr:       pointer,
		tlsConfig: cloneTLSConfig(s.tlsConfig),
	}
	runtime.SetFinalizer(handle, (*ServerHandle).finalize)
	return handle, nil
}

// Stop stops the native server but keeps the wrapper allocated until Close.
func (s *Server) Stop() error {
	if s == nil {
		return nil
	}
	s.lifecycleMu.Lock()
	defer s.lifecycleMu.Unlock()

	s.mu.Lock()
	pointer := s.ptr
	if pointer == nil {
		s.mu.Unlock()
		return nil
	}
	s.stopping = true
	s.mu.Unlock()

	err := statusError(s.api.serverStop(pointer))
	s.mu.Lock()
	s.stopping = false
	s.mu.Unlock()
	return err
}

// Close destroys the native server and then releases callback registrations.
func (s *Server) Close() error {
	if s == nil {
		return nil
	}
	s.lifecycleMu.Lock()
	defer s.lifecycleMu.Unlock()

	s.mu.Lock()
	pointer := s.ptr
	s.ptr = nil
	s.stopping = true
	handles := s.handles
	s.handles = nil
	s.mu.Unlock()
	runtime.SetFinalizer(s, nil)
	if pointer == nil {
		return nil
	}

	// mma_server_destroy calls stop and joins the runtime thread. Only after it
	// returns can callback IDs be removed from callbackTable.
	s.api.serverDestroy(pointer)
	for _, id := range handles {
		releaseCallback(id)
	}
	return nil
}

// ServerHandle is independently owned by the native server runtime.
type ServerHandle struct {
	mu        sync.Mutex
	api       *nativeAPI
	ptr       unsafe.Pointer
	tlsConfig *TLSConfig
}

func (h *ServerHandle) finalize() { _ = h.Close() }

func (h *ServerHandle) Close() error {
	if h == nil {
		return nil
	}
	h.mu.Lock()
	pointer := h.ptr
	h.ptr = nil
	h.mu.Unlock()
	runtime.SetFinalizer(h, nil)
	if pointer != nil {
		h.api.serverHandleDestroy(pointer)
	}
	return nil
}

// SendToChannel sends a message to an established channel connection. A
// connectionID of zero broadcasts according to the native C ABI contract.
func (h *ServerHandle) SendToChannel(connectionID uint32, message Response) error {
	if h == nil {
		return ErrClosed
	}
	h.mu.Lock()
	defer h.mu.Unlock()
	if h.ptr == nil {
		return ErrClosed
	}

	response, err := buildNativeResponse(h.api, message)
	if err != nil {
		return err
	}
	defer h.api.responseDestroy(response)

	return statusError(h.api.serverHandleSendToChannel(h.ptr, connectionID, response))
}

// ChangeConfig applies a configuration update while preserving the handle's
// current TLS settings.
func (h *ServerHandle) ChangeConfig(config ServerConfig) error {
	if h == nil {
		return ErrClosed
	}
	h.mu.Lock()
	defer h.mu.Unlock()
	if h.ptr == nil {
		return ErrClosed
	}
	return h.changeConfigLocked(config, h.tlsConfig)
}

func (h *ServerHandle) changeConfigLocked(config ServerConfig, tls *TLSConfig) error {
	nativeConfig, bindIP, nativeTLS, certificatePath, keyPath, err := buildNativeConfig(config, tls)
	if err != nil {
		return err
	}
	err = statusError(h.api.serverHandleChangeConfig(h.ptr, unsafe.Pointer(&nativeConfig)))
	keepNativeConfigAlive(&nativeConfig, bindIP, nativeTLS, certificatePath, keyPath)
	return err
}

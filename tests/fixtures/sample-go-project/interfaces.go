package main

type Handler interface {
	ServeHTTP(w ResponseWriter, r *Request)
}

type ReadWriter interface {
	Read(p []byte) (n int, err error)
	Write(p []byte) (n int, err error)
}

type MyHandler struct {
	Name string
}

func (h *MyHandler) ServeHTTP(w ResponseWriter, r *Request) {
	fmt.Fprintf(w, "Hello from %s", h.Name)
}

package main

import (
	"fmt"
	"net/http"
)

const MaxRetries = 3

var DefaultTimeout = 30

type UserID string

type Server struct {
	Host string
	Port int
}

func (s *Server) Start() error {
	fmt.Println("Starting server")
	return http.ListenAndServe(s.Address(), nil)
}

func (s *Server) Address() string {
	return fmt.Sprintf("%s:%d", s.Host, s.Port)
}

func NewServer(host string, port int) *Server {
	return &Server{Host: host, Port: port}
}

func main() {
	srv := NewServer("localhost", 8080)
	go srv.Start()
	defer fmt.Println("Server stopped")
	fmt.Println("Server running")
}

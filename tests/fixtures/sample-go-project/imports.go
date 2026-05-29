package main

import "fmt"

import (
	"net/http"
	h "net/http"
	. "strings"
	"encoding/json"
)

func demo() {
	fmt.Println("hello")
	h.Get("http://example.com")
	HasPrefix("hello", "he")
	json.Marshal(nil)
}

// Command shop wires the services together.
package main

import (
	"fmt"
	"os"

	"example.com/shop/config"
	"example.com/shop/model"
	"example.com/shop/report"
	"example.com/shop/service"
	"example.com/shop/store"
)

func handler(event []string) map[string][]string {
	return map[string][]string{"handled": event}
}

func dispatch(event []string) map[string][]string {
	handler := func(queued []string) map[string][]string {
		return map[string][]string{"queued": queued}
	}
	return handler(event)
}

func main() {
	fmt.Println(run(os.Args[1:]))
}

func run(argv []string) string {
	users := service.NewUserService(store.NewRepo[*model.User]())
	orders := service.NewOrderService(store.NewRepo[*model.Order]())
	user, err := users.Register(argv[0], argv[1])
	if err != nil {
		return err.Error()
	}
	orders.Place(user, []model.Item{{Price: 3}})
	fmt.Println(model.FormatName(user.Name))
	dispatch(argv)
	handler(argv)
	report.Publish(report.NewOrderReport(nil).Report, report.ConsoleSink{})
	return config.New(map[string]string{"argv": argv[0]}).Get("argv", "")
}

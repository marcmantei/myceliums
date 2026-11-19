// Package model holds the domain objects.
package model

import (
	"strings"

	"example.com/shop/store"
)

// Item is one order line.
type Item struct {
	Price int
}

// User is a customer.
type User struct {
	Name  string
	Email string
}

// Save stores the user.
func (u *User) Save(repo *store.Repo[*User]) *User {
	repo.Add(u)
	return u
}

// DisplayName is the formatted name.
func (u *User) DisplayName() string {
	return FormatName(u.Name)
}

// Order is a purchase.
type Order struct {
	User  *User
	Items []Item
}

// NewOrder builds an order.
func NewOrder(user *User, items []Item) *Order {
	return &Order{User: user, Items: items}
}

// Save stores the order.
func (o *Order) Save(repo *store.Repo[*Order]) *Order {
	repo.Add(o)
	return o
}

// Total sums the item prices.
func (o *Order) Total() int {
	total := 0
	for _, item := range o.Items {
		total += item.Price
	}
	return total
}

// Add appends an item.
func (o *Order) Add(item Item) {
	o.Items = append(o.Items, item)
}

// FormatName normalises a display name.
func FormatName(name string) string {
	return strings.ToUpper(strings.TrimSpace(name))
}

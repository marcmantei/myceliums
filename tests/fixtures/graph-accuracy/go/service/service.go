// Package service holds the application services.
package service

import (
	"errors"

	tok "example.com/shop/auth"
	"example.com/shop/config"
	"example.com/shop/model"
	"example.com/shop/store"
	"example.com/shop/validation"
)

// UserService manages users.
type UserService struct {
	repo      *store.Repo[*model.User]
	validator *validation.Validator
}

// NewUserService wires a UserService.
func NewUserService(repo *store.Repo[*model.User]) *UserService {
	return &UserService{repo: repo, validator: validation.NewValidator(validation.IsEmail)}
}

// Register creates and stores a user.
func (s *UserService) Register(name, email string) (*model.User, error) {
	if !s.validator.Validate(email) {
		return nil, errors.New("invalid email")
	}
	user := &model.User{Name: name, Email: email}
	return user.Save(s.repo), nil
}

// Login parses and refreshes a token.
func (s *UserService) Login(raw string) (*tok.Token, error) {
	token, err := tok.Parse(raw)
	if err != nil {
		return nil, err
	}
	return tok.Refresh(token, 0), nil
}

// Settings reads the theme.
func (s *UserService) Settings(text string) string {
	cfg, err := config.Load(text)
	if err != nil {
		return "light"
	}
	return cfg.Get("theme", "light")
}

// Get returns a stored user.
func (s *UserService) Get(index int) *model.User {
	return s.repo.Get(index)
}

// OrderService manages orders.
type OrderService struct {
	repo *store.Repo[*model.Order]
}

// NewOrderService wires an OrderService.
func NewOrderService(repo *store.Repo[*model.Order]) *OrderService {
	return &OrderService{repo: repo}
}

// Place creates, stores and totals an order.
func (s *OrderService) Place(user *model.User, items []model.Item) int {
	order := model.NewOrder(user, items)
	order.Save(s.repo)
	return order.Total()
}

// Parse reads the currency setting.
func (s *OrderService) Parse(text string) string {
	values, err := config.Parse(text)
	if err != nil {
		return "EUR"
	}
	return values["currency"]
}

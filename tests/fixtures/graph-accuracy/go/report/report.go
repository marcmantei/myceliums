// Package report renders plain-text reports.
package report

import (
	"fmt"
	"strings"

	"example.com/shop/model"
)

// Sink receives rendered text.
type Sink interface {
	Write(text string)
}

// ConsoleSink prints.
type ConsoleSink struct{}

// Write prints text.
func (ConsoleSink) Write(text string) {
	fmt.Println(text)
}

// MemorySink collects lines.
type MemorySink struct {
	Lines []string
}

// Write keeps text.
func (m *MemorySink) Write(text string) {
	m.Lines = append(m.Lines, text)
}

// Render upper-cases a title.
func Render(text string) string {
	return strings.ToUpper(text)
}

// Join joins cells of a row.
func Join(parts ...string) string {
	return strings.Join(parts, " | ")
}

// Report is a titled list of rows.
type Report struct {
	Title string
}

// Header is the rendered title.
func (r *Report) Header() string {
	return Render(r.Title)
}

// Render renders the header and rows.
func (r *Report) Render(rows []string) string {
	lines := []string{r.Header()}
	for _, row := range rows {
		lines = append(lines, r.RenderRow(row))
	}
	return strings.Join(lines, "\n")
}

// RenderRow renders one row.
func (r *Report) RenderRow(row string) string {
	return row
}

// OrderReport is a Report over orders.
type OrderReport struct {
	*Report
	orders []*model.Order
}

// NewOrderReport builds an OrderReport.
func NewOrderReport(orders []*model.Order) *OrderReport {
	return &OrderReport{Report: &Report{Title: "Orders"}, orders: orders}
}

// RenderRow renders one order.
func (o *OrderReport) RenderRow(order *model.Order) string {
	return Join(model.FormatName(order.User.Name), fmt.Sprint(order.Total()))
}

// Summary renders every order.
func (o *OrderReport) Summary() string {
	rows := make([]string, 0, len(o.orders))
	for _, order := range o.orders {
		rows = append(rows, o.RenderRow(order))
	}
	return o.Render(rows)
}

// Publish writes a rendered report to a sink.
func Publish(r *Report, sink Sink) {
	sink.Write(r.Render(nil))
}

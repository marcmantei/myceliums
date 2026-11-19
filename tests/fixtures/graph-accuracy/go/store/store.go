// Package store keeps rows in memory.
package store

// Repo keeps rows in memory.
type Repo[T any] struct {
	rows []T
}

// NewRepo returns an empty repository.
func NewRepo[T any]() *Repo[T] {
	return &Repo[T]{}
}

// Add appends a row and returns the new count.
func (r *Repo[T]) Add(row T) int {
	r.rows = append(r.rows, row)
	return len(r.rows)
}

// Get returns the row at index.
func (r *Repo[T]) Get(index int) T {
	return r.rows[index]
}

// Find returns the first row matching predicate.
func (r *Repo[T]) Find(predicate func(T) bool) (T, bool) {
	for _, row := range r.rows {
		if predicate(row) {
			return row, true
		}
	}
	var zero T
	return zero, false
}

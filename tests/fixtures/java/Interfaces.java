package com.example.app;

public interface Repository<T> {
    T findById(String id);
    void save(T entity);
    void delete(String id);
}

class UserRepository implements Repository<User> {
    public User findById(String id) {
        return new User(id);
    }

    public void save(User entity) {
        System.out.println("Saving: " + entity);
    }

    public void delete(String id) {
        System.out.println("Deleting: " + id);
    }
}

record Point(int x, int y) {
    public double distanceTo(Point other) {
        return Math.sqrt(Math.pow(x - other.x, 2) + Math.pow(y - other.y, 2));
    }
}

@interface CustomAnnotation {
}
